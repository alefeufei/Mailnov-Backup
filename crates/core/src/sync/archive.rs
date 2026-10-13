use crate::store::Store;
use crate::{Error, Event, MailSource, RawMessage, Result, RunSummary};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::mpsc::Sender;

pub struct ArchiveOptions {
    pub account: String,
    pub folders: Option<Vec<String>>,
    pub since: Option<i64>,
    pub batch_size: usize,
}

const DEFAULT_BATCH: usize = 50;

fn emit(events: &Sender<Event>, e: Event) {
    // A closed receiver must never make the archive fail.
    let _ = events.send(e);
}

/// Why one message could not be archived. The original error is kept so the decision to go on
/// or to stop is made on its class, never on how many messages failed.
enum MessageFailure {
    /// The source rejected the fetch of this message (a non-systemic error, e.g. `NO`/`BAD`).
    Fetch(Error),
    /// The fetch succeeded but this UID, still listed, was not in the response.
    MissingFromResponse,
}

impl MessageFailure {
    fn reason(&self) -> String {
        match self {
            MessageFailure::Fetch(e) => e.to_string(),
            MessageFailure::MissingFromResponse => "missing from fetch response".to_string(),
        }
    }
}

/// Messages obtained, and the UIDs that failed with their reason.
type Fetched = (Vec<RawMessage>, Vec<(u32, MessageFailure)>);

/// Fetch `uids`; on a batch error retry one UID at a time to isolate bad messages.
/// Returns the messages obtained and the UIDs that failed with their error. A systemic error
/// (connection, login, disk) at any point is returned as `Err` at once: the run must stop, and
/// retrying every remaining UID against a dead connection would only produce a failure storm.
fn fetch_isolating(source: &mut dyn MailSource, uids: &[u32]) -> Result<Fetched> {
    match source.fetch(uids) {
        Ok(msgs) => Ok((msgs, vec![])),
        Err(e) if is_systemic(&e) => Err(e),
        // A single UID was already tried on its own.
        Err(e) if uids.len() == 1 => Ok((vec![], vec![(uids[0], MessageFailure::Fetch(e))])),
        Err(_) => {
            let mut ok = vec![];
            let mut failed = vec![];
            for &uid in uids {
                match source.fetch(&[uid]) {
                    Ok(mut m) => ok.append(&mut m),
                    Err(e) if is_systemic(&e) => return Err(e),
                    Err(e) => failed.push((uid, MessageFailure::Fetch(e))),
                }
            }
            Ok((ok, failed))
        }
    }
}

/// An error that ended the processing of one folder.
struct FolderError {
    error: Error,
    /// Stop the whole run (connection/login/disk/index trouble) instead of going on with the
    /// next folder.
    abort: bool,
}

impl From<Error> for FolderError {
    fn from(error: Error) -> Self {
        let abort = is_systemic(&error);
        FolderError { error, abort }
    }
}

/// Failures that concern the whole connection, login or local archive rather than one folder.
fn is_systemic(e: &Error) -> bool {
    matches!(
        e,
        Error::Connection(_) | Error::Auth(_) | Error::Io(_) | Error::Db(_)
    )
}

/// `INBOX` is case-insensitive in IMAP (also as the first level of a hierarchy).
fn normalize_folder(name: &str) -> String {
    match name.split_once('/') {
        Some((first, rest)) if first.eq_ignore_ascii_case("INBOX") => format!("INBOX/{rest}"),
        None if name.eq_ignore_ascii_case("INBOX") => "INBOX".to_string(),
        _ => name.to_string(),
    }
}

/// Archive every folder of `source` (or only `opts.folders`) into `store`.
///
/// A folder that cannot be processed (not found on the server, `EXAMINE`/search refused, any
/// other per-folder server error) emits `FolderFailed`, counts as one failure and the run goes
/// on with the next folder. A message the source refuses to deliver is a `MessageFailed` (and
/// pins `last_uid` below it), however many of them there are. Connection, login, disk and index
/// errors (including one hit while fetching a single message) end the run at once with `Err`.
pub fn archive(
    source: &mut dyn MailSource,
    store: &mut dyn Store,
    opts: &ArchiveOptions,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    let batch_size = if opts.batch_size == 0 {
        DEFAULT_BATCH
    } else {
        opts.batch_size
    };
    let mut summary = RunSummary::default();

    let mut folders = source.list_folders()?;
    if let Some(wanted) = &opts.folders {
        let wanted_norm: Vec<String> = wanted.iter().map(|w| normalize_folder(w)).collect();
        let listed_norm: HashSet<String> = folders.iter().map(|f| normalize_folder(f)).collect();
        folders.retain(|f| wanted_norm.contains(&normalize_folder(f)));
        let mut reported = HashSet::new();
        for (w, n) in wanted.iter().zip(&wanted_norm) {
            if !listed_norm.contains(n) && reported.insert(n.clone()) {
                summary.failed += 1;
                emit(
                    events,
                    Event::FolderFailed {
                        folder: w.clone(),
                        reason: "folder not found".to_string(),
                    },
                );
            }
        }
    }

    for folder in folders {
        if let Err(FolderError { error, abort }) = archive_folder(
            source,
            store,
            opts,
            events,
            &folder,
            batch_size,
            &mut summary,
        ) {
            if abort {
                return Err(error);
            }
            summary.failed += 1;
            emit(
                events,
                Event::FolderFailed {
                    folder: folder.clone(),
                    reason: error.to_string(),
                },
            );
        }
    }

    emit(events, Event::Finished(summary.clone()));
    Ok(summary)
}

fn archive_folder(
    source: &mut dyn MailSource,
    store: &mut dyn Store,
    opts: &ArchiveOptions,
    events: &Sender<Event>,
    folder: &str,
    batch_size: usize,
    summary: &mut RunSummary,
) -> std::result::Result<(), FolderError> {
    let account = opts.account.as_str();
    let state = source.select(folder)?;
    let stored = store.folder_state(account, folder)?;
    let reconcile =
        matches!(&stored, Some(s) if s.uidvalidity != 0 && s.uidvalidity != state.uidvalidity);
    let base_last = if reconcile {
        0
    } else {
        stored.as_ref().map_or(0, |s| s.last_uid)
    };

    // UIDs committed under the current UIDVALIDITY (before this run): never downloaded again,
    // and their flags are refreshed.
    let known: Vec<u32> = if reconcile || stored.is_none() {
        vec![]
    } else {
        store
            .messages(account)?
            .into_iter()
            .filter(|m| m.folder == folder)
            .filter_map(|m| m.uid)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    };
    let known_set: HashSet<u32> = known.iter().copied().collect();

    // Everything listed above `base_last`. `last_uid` may sit below UIDs that were already
    // committed (pinned under a failed UID, or reset after a `since` change): those are skipped
    // below, while the full listing still lets `last_uid` move up past them.
    let listed = source.uids_after(base_last, opts.since)?;
    let total = listed.iter().filter(|u| !known_set.contains(u)).count();
    emit(
        events,
        Event::FolderStarted {
            folder: folder.to_string(),
            total,
        },
    );

    let mut last_uid = base_last;
    // Lowest UID that failed in this run: last_uid may never reach it.
    let mut lowest_failed: Option<u32> = None;
    // While reconciling, the stored UIDVALIDITY stays the old one until the folder is fully
    // processed without failures, so an interrupted run reconciles again.
    let commit_validity = if reconcile {
        stored.as_ref().map_or(state.uidvalidity, |s| s.uidvalidity)
    } else {
        state.uidvalidity
    };
    let mut folder_failed = false;

    for chunk in listed.chunks(batch_size) {
        let chunk_max = *chunk.iter().max().expect("chunks are non-empty");

        let wanted: Vec<u32> = if reconcile {
            let answers: HashMap<u32, Option<String>> =
                source.message_ids(chunk)?.into_iter().collect();
            let mut keep = vec![];
            for &uid in chunk {
                // No answer, or no Message-ID: unknown, so download it. Only this folder's own
                // (old) locations count: a copy elsewhere in the account does not.
                let known_msg = match answers.get(&uid) {
                    Some(Some(id)) => store.has_message_id(account, folder, id)?,
                    _ => false,
                };
                if !known_msg {
                    keep.push(uid);
                }
            }
            keep
        } else {
            chunk
                .iter()
                .copied()
                .filter(|u| !known_set.contains(u))
                .collect()
        };

        // A systemic error (dead connection...) aborts the whole run right here.
        let (messages, mut failed) = if wanted.is_empty() {
            (vec![], vec![])
        } else {
            fetch_isolating(source, &wanted)?
        };

        // A UID absent from the response is expunged only if the source confirms it is gone.
        let got: HashSet<u32> = messages.iter().map(|m| m.uid).collect();
        let already_failed: HashSet<u32> = failed.iter().map(|(u, _)| *u).collect();
        let missing: Vec<u32> = wanted
            .iter()
            .copied()
            .filter(|u| !got.contains(u) && !already_failed.contains(u))
            .collect();
        if let Some(&min_missing) = missing.iter().min() {
            let still: HashSet<u32> = source
                .uids_after(min_missing - 1, opts.since)?
                .into_iter()
                .collect();
            for uid in missing {
                if still.contains(&uid) {
                    failed.push((uid, MessageFailure::MissingFromResponse));
                }
            }
            failed.sort_by_key(|(u, _)| *u);
        }

        for (uid, why) in &failed {
            summary.failed += 1;
            folder_failed = true;
            lowest_failed = Some(lowest_failed.map_or(*uid, |l| l.min(*uid)));
            emit(
                events,
                Event::MessageFailed {
                    folder: folder.to_string(),
                    uid: *uid,
                    reason: why.reason(),
                },
            );
        }
        let mut ceiling = chunk_max;
        if let Some(l) = lowest_failed {
            ceiling = ceiling.min(l.saturating_sub(1));
        }
        let new_last = last_uid.max(ceiling);
        if messages.is_empty() && new_last == last_uid {
            // Nothing to write: every UID of the chunk was skipped or failed.
            continue;
        }

        let outcome = store.commit_batch(account, folder, commit_validity, &messages, new_last)?;
        last_uid = new_last;
        summary.added += outcome.added;
        summary.duplicates += outcome.duplicates;
        for m in &messages {
            emit(
                events,
                Event::MessageArchived {
                    folder: folder.to_string(),
                    uid: m.uid,
                },
            );
        }
    }

    if reconcile && !folder_failed {
        // Whole folder reconciled: now persist the new UIDVALIDITY.
        store.commit_batch(account, folder, state.uidvalidity, &[], last_uid)?;
    }

    Ok(refresh_flags(
        source, store, account, folder, &known, batch_size,
    )?)
}

fn refresh_flags(
    source: &mut dyn MailSource,
    store: &mut dyn Store,
    account: &str,
    folder: &str,
    known: &[u32],
    batch_size: usize,
) -> Result<()> {
    if known.is_empty() {
        return Ok(());
    }
    let current: HashMap<u32, Vec<String>> = store
        .messages(account)?
        .into_iter()
        .filter(|m| m.folder == folder)
        .filter_map(|m| m.uid.map(|u| (u, m.flags)))
        .collect();
    for chunk in known.chunks(batch_size) {
        for (uid, flags) in source.flags(chunk)? {
            if current.get(&uid) != Some(&flags) {
                store.update_flags(account, folder, uid, &flags)?;
            }
        }
    }
    Ok(())
}
