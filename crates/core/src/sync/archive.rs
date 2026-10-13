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

/// Fetch `uids`; on a batch error retry one UID at a time to isolate bad messages.
/// Returns the messages obtained and the `(uid, reason)` of those that failed.
fn fetch_isolating(source: &mut dyn MailSource, uids: &[u32]) -> (Vec<RawMessage>, Vec<(u32, String)>) {
    match source.fetch(uids) {
        Ok(msgs) => (msgs, vec![]),
        Err(_) => {
            let mut ok = vec![];
            let mut failed = vec![];
            for &uid in uids {
                match source.fetch(&[uid]) {
                    Ok(mut m) => ok.append(&mut m),
                    Err(e) => failed.push((uid, e.to_string())),
                }
            }
            (ok, failed)
        }
    }
}

pub fn archive(
    source: &mut dyn MailSource,
    store: &mut dyn Store,
    opts: &ArchiveOptions,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    let batch_size = if opts.batch_size == 0 { DEFAULT_BATCH } else { opts.batch_size };
    let mut summary = RunSummary::default();

    let mut folders = source.list_folders()?;
    if let Some(wanted) = &opts.folders {
        folders.retain(|f| wanted.contains(f));
    }

    for folder in folders {
        archive_folder(source, store, opts, events, &folder, batch_size, &mut summary)?;
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
) -> Result<()> {
    let account = opts.account.as_str();
    let state = source.select(folder)?;
    let stored = store.folder_state(account, folder)?;
    let reconcile = matches!(&stored, Some(s) if s.uidvalidity != 0 && s.uidvalidity != state.uidvalidity);
    let base_last = if reconcile { 0 } else { stored.as_ref().map_or(0, |s| s.last_uid) };

    // UIDs already known to the store (before this run), for flag refresh.
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

    let uids = source.uids_after(base_last, opts.since)?;
    emit(events, Event::FolderStarted { folder: folder.to_string(), total: uids.len() });

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

    for chunk in uids.chunks(batch_size) {
        let chunk_max = *chunk.iter().max().expect("chunks are non-empty");

        let wanted: Vec<u32> = if reconcile {
            let answers: HashMap<u32, Option<String>> = source.message_ids(chunk)?.into_iter().collect();
            let mut keep = vec![];
            for &uid in chunk {
                // No answer, or no Message-ID: unknown, so download it.
                let known_msg = match answers.get(&uid) {
                    Some(Some(id)) => store.has_message_id(account, id)?,
                    _ => false,
                };
                if !known_msg {
                    keep.push(uid);
                }
            }
            keep
        } else {
            chunk.to_vec()
        };

        let (messages, mut failed) =
            if wanted.is_empty() { (vec![], vec![]) } else { fetch_isolating(source, &wanted) };

        // A UID absent from the response is expunged only if the source confirms it is gone.
        let got: HashSet<u32> = messages.iter().map(|m| m.uid).collect();
        let already_failed: HashSet<u32> = failed.iter().map(|(u, _)| *u).collect();
        let missing: Vec<u32> =
            wanted.iter().copied().filter(|u| !got.contains(u) && !already_failed.contains(u)).collect();
        if let Some(&min_missing) = missing.iter().min() {
            let listed: HashSet<u32> =
                source.uids_after(min_missing - 1, opts.since)?.into_iter().collect();
            for uid in missing {
                if listed.contains(&uid) {
                    failed.push((uid, "missing from fetch response".to_string()));
                }
            }
            failed.sort_by_key(|(u, _)| *u);
        }

        for (uid, reason) in &failed {
            summary.failed += 1;
            folder_failed = true;
            lowest_failed = Some(lowest_failed.map_or(*uid, |l| l.min(*uid)));
            emit(
                events,
                Event::MessageFailed { folder: folder.to_string(), uid: *uid, reason: reason.clone() },
            );
        }
        if !wanted.is_empty() && failed.len() == wanted.len() {
            return Err(Error::Imap(format!(
                "every message of the batch in {folder} failed to download"
            )));
        }

        let mut ceiling = chunk_max;
        if let Some(l) = lowest_failed {
            ceiling = ceiling.min(l.saturating_sub(1));
        }
        let new_last = last_uid.max(ceiling);

        let outcome = store.commit_batch(account, folder, commit_validity, &messages, new_last)?;
        last_uid = new_last;
        summary.added += outcome.added;
        summary.duplicates += outcome.duplicates;
        for m in &messages {
            emit(events, Event::MessageArchived { folder: folder.to_string(), uid: m.uid });
        }
    }

    if reconcile && !folder_failed {
        // Whole folder reconciled: now persist the new UIDVALIDITY.
        store.commit_batch(account, folder, state.uidvalidity, &[], last_uid)?;
    }

    refresh_flags(source, store, account, folder, &known, batch_size)
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
