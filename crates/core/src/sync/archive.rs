use crate::store::Store;
use crate::{Error, Event, MailSource, RawMessage, Result, RunSummary};
use std::collections::{BTreeSet, HashMap};
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

    for chunk in uids.chunks(batch_size) {
        let chunk_max = *chunk.iter().max().expect("chunks are non-empty");

        let wanted: Vec<u32> = if reconcile {
            let ids = source.message_ids(chunk)?;
            let mut keep = vec![];
            for (uid, mid) in ids {
                let known_msg = match mid {
                    Some(id) => store.has_message_id(account, &id)?,
                    None => false,
                };
                if !known_msg {
                    keep.push(uid);
                }
            }
            keep
        } else {
            chunk.to_vec()
        };

        let (messages, failed) =
            if wanted.is_empty() { (vec![], vec![]) } else { fetch_isolating(source, &wanted) };

        for (uid, reason) in &failed {
            summary.failed += 1;
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

        let outcome = store.commit_batch(account, folder, state.uidvalidity, &messages, new_last)?;
        last_uid = new_last;
        summary.added += outcome.added;
        summary.duplicates += outcome.duplicates;
        for m in &messages {
            emit(events, Event::MessageArchived { folder: folder.to_string(), uid: m.uid });
        }
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
