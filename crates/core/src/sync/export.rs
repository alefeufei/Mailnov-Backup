use crate::store::{Store, StoredMessage};
use crate::target::MailTarget;
use crate::{Error, Event, Result, RunSummary};
use std::collections::HashSet;
use std::sync::mpsc::Sender;

/// This many consecutive connection-level failures mean the destination is down and the run
/// stops instead of hammering it with every remaining message.
const GIVE_UP_AFTER: usize = 3;

fn emit(events: &Sender<Event>, e: Event) {
    // A closed receiver must never make the export fail.
    let _ = events.send(e);
}

/// Failures that say something about the destination or the local disk as a whole rather than
/// about one message or folder (a server `NO`/`BAD` is `Error::Imap` and never counts).
fn is_systemic(e: &Error) -> bool {
    matches!(e, Error::Connection(_) | Error::Auth(_) | Error::Io(_))
}

/// Copy every archived message of `account` that `target` has not received yet.
///
/// A message is recorded as exported (`mark_exported`) only after `put` returned `Ok`, so a
/// failed message is retried by the next run. The same message listed twice for one folder (a
/// row with a UID and one without, after a UIDVALIDITY reconcile) is sent once.
///
/// Per-message and per-folder failures are reported (`MessageFailed`, counted in `failed`) and
/// the run goes on. It stops with an error only for a rejected login (`Error::Auth`, at once) or
/// after 3 consecutive connection-level failures (`Error::Connection`/`Io`); `Finished` is
/// emitted before returning that error so the progress made is not lost. A local `.eml` that is
/// missing or does not match its hash is `Error::Corrupt`: that message fails, is never sent, and
/// the run goes on.
pub fn export(
    store: &mut dyn Store,
    target: &mut dyn MailTarget,
    account: &str,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    let target_id = target.id();
    let mut summary = RunSummary::default();

    // Group by folder (first-seen order), one entry per (sha256, folder).
    let mut folders: Vec<(String, Vec<StoredMessage>)> = vec![];
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for m in store.messages(account)? {
        if !seen.insert((m.sha256.clone(), m.folder.clone())) {
            continue;
        }
        match folders.iter_mut().find(|(f, _)| *f == m.folder) {
            Some((_, list)) => list.push(m),
            None => folders.push((m.folder.clone(), vec![m])),
        }
    }

    let mut streak = 0usize;
    let mut fatal: Option<Error> = None;
    'folders: for (folder, msgs) in folders {
        let mut pending = vec![];
        for m in msgs {
            if store.export_done(&target_id, &m.sha256, &folder)? {
                summary.duplicates += 1;
            } else {
                pending.push(m);
            }
        }
        if pending.is_empty() {
            continue;
        }
        emit(
            events,
            Event::FolderStarted {
                folder: folder.clone(),
                total: pending.len(),
            },
        );

        // A folder that cannot be created fails only its own messages; they do not count toward
        // the abort streak (the destination answered), except a rejected login.
        if let Err(e) = target.ensure_folder(&folder) {
            let reason = format!("folder could not be created: {e}");
            for m in &pending {
                summary.failed += 1;
                emit(
                    events,
                    Event::MessageFailed {
                        folder: folder.clone(),
                        uid: m.uid.unwrap_or(0),
                        reason: reason.clone(),
                    },
                );
            }
            if matches!(e, Error::Auth(_)) {
                fatal = Some(e);
                break 'folders;
            }
            continue;
        }

        for m in pending {
            let uid = m.uid.unwrap_or(0);
            let outcome = store
                .read_eml(&m.sha256)
                .and_then(|bytes| target.put(&folder, &bytes, &m.flags, m.internal_date));
            match outcome {
                Ok(()) => {
                    // Only now is the message known to be at the destination.
                    store.mark_exported(&target_id, &m.sha256, &folder)?;
                    streak = 0;
                    summary.added += 1;
                    emit(
                        events,
                        Event::MessageArchived {
                            folder: folder.clone(),
                            uid,
                        },
                    );
                }
                Err(e) => {
                    summary.failed += 1;
                    emit(
                        events,
                        Event::MessageFailed {
                            folder: folder.clone(),
                            uid,
                            reason: e.to_string(),
                        },
                    );
                    if matches!(e, Error::Auth(_)) {
                        fatal = Some(e);
                        break 'folders;
                    }
                    if is_systemic(&e) {
                        streak += 1;
                        if streak >= GIVE_UP_AFTER {
                            fatal = Some(e);
                            break 'folders;
                        }
                    } else {
                        streak = 0;
                    }
                }
            }
        }
    }

    emit(events, Event::Finished(summary.clone()));
    match fatal {
        Some(e) => Err(e),
        None => Ok(summary),
    }
}
