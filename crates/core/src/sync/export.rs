use crate::store::{Store, StoredMessage};
use crate::target::MailTarget;
use crate::{Error, Event, Result, RunSummary};
use std::collections::HashSet;
use std::mem::{discriminant, Discriminant};
use std::sync::mpsc::Sender;

/// With no success at all, this many failures of one kind in a row mean the destination is down
/// and the run stops instead of hammering it with every remaining message.
const GIVE_UP_AFTER: usize = 3;

fn emit(events: &Sender<Event>, e: Event) {
    // A closed receiver must never make the export fail.
    let _ = events.send(e);
}

/// Tracks whether the run is failing wholesale (every attempt failed with the same kind of error).
#[derive(Default)]
struct Health {
    attempted: usize,
    succeeded: usize,
    kind: Option<Discriminant<Error>>,
    mixed: bool,
    last: Option<Error>,
}

impl Health {
    fn ok(&mut self) {
        self.attempted += 1;
        self.succeeded += 1;
    }

    fn failed(&mut self, e: Error) {
        self.attempted += 1;
        let k = discriminant(&e);
        match self.kind {
            None => self.kind = Some(k),
            Some(prev) if prev != k => self.mixed = true,
            _ => {}
        }
        self.last = Some(e);
    }

    fn all_failed_alike(&self) -> bool {
        self.succeeded == 0 && !self.mixed && self.last.is_some()
    }

    fn failures(&self) -> usize {
        self.attempted - self.succeeded
    }
}

/// Copy every archived message of `account` that `target` has not received yet.
///
/// A message is recorded as exported (`mark_exported`) only after `put` returned `Ok`, so a
/// failed message is retried by the next run. The same message listed twice for one folder (a
/// row with a UID and one without, after a UIDVALIDITY reconcile) is sent once.
///
/// A failing message never aborts the run, unless nothing at all succeeds and every failure is
/// the same kind of error (a dead destination): then the run stops with that error.
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

    let mut health = Health::default();
    for (folder, msgs) in folders {
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

        let mut folder_error: Option<String> = None;
        if let Err(e) = target.ensure_folder(&folder) {
            folder_error = Some(e.to_string());
            // Counted once here (it fixes the error kind); each of its messages repeats it below.
            health.failed(e);
            health.attempted -= 1;
        }

        for m in pending {
            let uid = m.uid.unwrap_or(0);
            let outcome = match &folder_error {
                Some(reason) => Err(Error::Imap(format!(
                    "folder could not be created: {reason}"
                ))),
                None => store
                    .read_eml(&m.sha256)
                    .and_then(|bytes| target.put(&folder, &bytes, &m.flags, m.internal_date)),
            };
            match outcome {
                Ok(()) => {
                    // Only now is the message known to be at the destination.
                    store.mark_exported(&target_id, &m.sha256, &folder)?;
                    health.ok();
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
                    let reason = e.to_string();
                    if folder_error.is_some() {
                        health.attempted += 1; // same error as the folder's, already recorded
                    } else {
                        health.failed(e);
                    }
                    emit(
                        events,
                        Event::MessageFailed {
                            folder: folder.clone(),
                            uid,
                            reason,
                        },
                    );
                }
            }
            if health.all_failed_alike() && health.failures() >= GIVE_UP_AFTER {
                return Err(health.last.take().expect("failures recorded"));
            }
        }
    }

    // A run whose every attempt failed alike (two or more attempts: one bad message alone is
    // just a bad message).
    if health.all_failed_alike() && health.attempted >= 2 {
        return Err(health.last.take().expect("failures recorded"));
    }

    emit(events, Event::Finished(summary.clone()));
    Ok(summary)
}
