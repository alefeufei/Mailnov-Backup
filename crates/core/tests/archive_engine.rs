use mailbackup_core::store::{
    BatchOutcome, FaultPoint, FsSqliteStore, Store, StoredFolder, StoredMessage, VerifyReport,
};
use mailbackup_core::sync::{archive, ArchiveOptions};
use mailbackup_core::testing::{FakeSource, Injected};
use mailbackup_core::{Error, Event, RawMessage, Result, RunSummary};
use proptest::prelude::*;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};

const ACC: &str = "me";

fn eml(n: u32) -> Vec<u8> {
    format!(
        "Message-ID: <m{n}@x>\r\nFrom: A <a@x.com>\r\nTo: b@x.com\r\nSubject: s{n}\r\nDate: Mon, 02 Jan 2006 15:04:05 +0000\r\n\r\nbody {n}\r\n"
    )
    .into_bytes()
}

fn opts(batch: usize) -> ArchiveOptions {
    ArchiveOptions {
        account: ACC.into(),
        folders: None,
        since: None,
        batch_size: batch,
    }
}

fn inbox(n: u32) -> FakeSource {
    let mut f = FakeSource::new();
    f.add_folder("INBOX", 7);
    for i in 1..=n {
        f.add_message("INBOX", i, eml(i), &[]);
    }
    f
}

fn run(
    src: &mut FakeSource,
    store: &mut dyn Store,
    o: &ArchiveOptions,
) -> (Result<RunSummary>, Vec<Event>) {
    let (tx, rx): (Sender<Event>, Receiver<Event>) = channel();
    let r = archive(src, store, o, &tx);
    drop(tx);
    (r, rx.try_iter().collect())
}

fn last_uid(s: &dyn Store, folder: &str) -> u32 {
    s.folder_state(ACC, folder).unwrap().unwrap().last_uid
}

fn walk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn go(base: &Path, d: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                go(base, &p, out)
            } else if p.extension().is_some_and(|x| x == "eml") {
                out.insert(
                    p.strip_prefix(base)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    std::fs::read(&p).unwrap(),
                );
            }
        }
    }
    let mut m = BTreeMap::new();
    go(root, &root.join("mail"), &mut m);
    m
}

fn norm(mut v: Vec<StoredMessage>) -> Vec<StoredMessage> {
    v.sort_by(|a, b| (&a.sha256, &a.folder, a.uid).cmp(&(&b.sha256, &b.folder, b.uid)));
    v
}

#[test]
fn first_run_archives_everything() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(120);
    let (r, events) = run(&mut src, &mut store, &opts(50));
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (120, 0));
    assert_eq!(last_uid(&store, "INBOX"), 120);
    assert_eq!(src.fetch_calls, 3);
    assert_eq!(store.messages(ACC).unwrap().len(), 120);
    assert!(matches!(
        events.first(),
        Some(Event::FolderStarted { total: 120, .. })
    ));
    assert!(matches!(events.last(), Some(Event::Finished(_))));
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::MessageArchived { .. }))
            .count(),
        120
    );
}

#[test]
fn second_run_fetches_only_new_uids() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    let before = src.fetch_calls;
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.added, 0);
    assert_eq!(src.fetch_calls, before);
    src.add_message("INBOX", 11, eml(11), &[]);
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.added, 1);
    assert_eq!(src.fetched_uids.last(), Some(&11));
}

#[test]
fn resume_after_failure_between_batches() {
    let ref_dir = tempfile::tempdir().unwrap();
    let mut ref_store = FsSqliteStore::open(ref_dir.path()).unwrap();
    run(&mut inbox(120), &mut ref_store, &opts(50)).0.unwrap();

    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(120);
    src.fail_after_fetches = Some(1);
    let (r, _) = run(&mut src, &mut store, &opts(50));
    assert!(matches!(r, Err(Error::Imap(_))));
    assert_eq!(last_uid(&store, "INBOX"), 50);
    src.clear_failures();
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(
        norm(store.messages(ACC).unwrap()),
        norm(ref_store.messages(ACC).unwrap())
    );
    assert_eq!(walk(dir.path()), walk(ref_dir.path()));
    assert_eq!(last_uid(&store, "INBOX"), 120);
}

#[test]
fn uidvalidity_change_reconciles_by_message_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(5);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    src.renumber("INBOX", 99, 100);
    src.add_message("INBOX", 200, eml(200), &[]);
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.added, 1);
    assert_eq!(src.fetched_uids, vec![200]);
    let msgs = store.messages(ACC).unwrap();
    let shas: std::collections::BTreeSet<_> = msgs.iter().map(|m| m.sha256.clone()).collect();
    assert_eq!(shas.len(), 6);
    assert_eq!(
        store.folder_state(ACC, "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 99,
            last_uid: 200
        })
    );
    // and a further run is a no-op
    let before = src.fetch_calls;
    assert_eq!(run(&mut src, &mut store, &opts(50)).0.unwrap().added, 0);
    assert_eq!(src.fetch_calls, before);
}

#[test]
fn uidvalidity_change_with_all_known_still_records_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(5);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    src.renumber("INBOX", 99, 100);
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.added, 0);
    assert!(src.fetched_uids.is_empty());
    assert_eq!(
        store.folder_state(ACC, "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 99,
            last_uid: 105
        })
    );
}

#[test]
fn bad_message_does_not_abort() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    src.fail_uids.insert(5);
    let (r, events) = run(&mut src, &mut store, &opts(50));
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (9, 1));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::MessageFailed { uid: 5, .. })));
    assert_eq!(store.messages(ACC).unwrap().len(), 9);
}

#[test]
fn last_uid_does_not_advance_past_a_failed_uid() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    src.fail_uids.insert(5);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(last_uid(&store, "INBOX"), 4);
    // across batches too: bad uid in batch 1 keeps later batches from advancing
    let dir2 = tempfile::tempdir().unwrap();
    let mut store2 = FsSqliteStore::open(dir2.path()).unwrap();
    let mut src2 = inbox(12);
    src2.fail_uids.insert(2);
    let s = run(&mut src2, &mut store2, &opts(4)).0.unwrap();
    assert_eq!((s.added, s.failed), (11, 1));
    assert_eq!(last_uid(&store2, "INBOX"), 1);

    // next run picks the message up once it is healthy again
    src.clear_failures();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.added, 1);
    assert_eq!(s.failed, 0);
    assert_eq!(last_uid(&store, "INBOX"), 10);
    assert_eq!(store.messages(ACC).unwrap().len(), 10);
}

#[test]
fn message_without_message_id_or_date_is_archived() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("INBOX", 1);
    src.add_message_dated("INBOX", 1, b"Subject: x\r\n\r\n".to_vec(), &[], None);
    src.add_message_dated("INBOX", 2, b"\r\n".to_vec(), &[], None);
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!((s.added, s.failed), (2, 0));
    // without Message-ID they are downloaded again after a UIDVALIDITY change only if unknown
    assert_eq!(store.messages(ACC).unwrap().len(), 2);
}

#[test]
fn deleted_at_source_stays_archived() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(5);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    src.remove_message("INBOX", 3);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(store.messages(ACC).unwrap().len(), 5);
    assert_eq!(
        store.verify().unwrap(),
        VerifyReport {
            checked: 5,
            missing: vec![],
            corrupt: vec![]
        }
    );
}

#[test]
fn flags_are_updated_for_known_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(3);
    run(&mut src, &mut store, &opts(2)).0.unwrap();
    assert!(store
        .messages(ACC)
        .unwrap()
        .iter()
        .all(|m| m.flags.is_empty()));
    src.set_flags("INBOX", 2, &["\\Seen"]);
    src.set_flags("INBOX", 3, &["\\Flagged"]);
    let before = src.fetch_calls;
    run(&mut src, &mut store, &opts(2)).0.unwrap();
    assert_eq!(src.fetch_calls, before);
    let m = store.messages(ACC).unwrap();
    let flags = |uid: u32| m.iter().find(|x| x.uid == Some(uid)).unwrap().flags.clone();
    assert_eq!(flags(1), Vec::<String>::new());
    assert_eq!(flags(2), vec!["\\Seen".to_string()]);
    assert_eq!(flags(3), vec!["\\Flagged".to_string()]);
}

#[test]
fn folder_filter_and_since_are_respected() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("INBOX", 1);
    src.add_folder("Sent", 1);
    src.add_message_dated("INBOX", 1, eml(1), &[], Some(100));
    src.add_message_dated("INBOX", 2, eml(2), &[], Some(200));
    src.add_message_dated("Sent", 1, eml(3), &[], Some(300));
    let o = ArchiveOptions {
        account: ACC.into(),
        folders: Some(vec!["INBOX".into()]),
        since: Some(150),
        batch_size: 50,
    };
    let s = run(&mut src, &mut store, &o).0.unwrap();
    assert_eq!(s.added, 1);
    assert_eq!(store.folder_state(ACC, "Sent").unwrap(), None);
    let m = store.messages(ACC).unwrap();
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].uid, Some(2));
}

#[test]
fn closed_receiver_does_not_fail_archive() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(3);
    let (tx, rx) = channel();
    drop(rx);
    let s = archive(&mut src, &mut store, &opts(0), &tx).unwrap();
    assert_eq!(s.added, 3);
}

/// Store wrapper that arms a one-shot fault on the n-th `commit_batch`.
struct Faulty {
    inner: FsSqliteStore,
    commits: usize,
    fail_at: usize,
    point: FaultPoint,
}

impl Store for Faulty {
    fn folder_state(&self, a: &str, f: &str) -> Result<Option<StoredFolder>> {
        self.inner.folder_state(a, f)
    }
    fn commit_batch(
        &mut self,
        a: &str,
        f: &str,
        v: u32,
        b: &[RawMessage],
        l: u32,
    ) -> Result<BatchOutcome> {
        if self.commits == self.fail_at {
            self.inner.inject_fault(self.point);
        }
        self.commits += 1;
        self.inner.commit_batch(a, f, v, b, l)
    }
    fn has_message_id(&self, a: &str, f: &str, m: &str) -> Result<bool> {
        self.inner.has_message_id(a, f, m)
    }
    fn update_flags(&mut self, a: &str, f: &str, u: u32, fl: &[String]) -> Result<()> {
        self.inner.update_flags(a, f, u, fl)
    }
    fn messages(&self, a: &str) -> Result<Vec<StoredMessage>> {
        self.inner.messages(a)
    }
    fn read_eml(&self, s: &str) -> Result<Vec<u8>> {
        self.inner.read_eml(s)
    }
    fn export_done(&self, t: &str, s: &str, f: &str) -> Result<bool> {
        self.inner.export_done(t, s, f)
    }
    fn mark_exported(&mut self, t: &str, s: &str, f: &str) -> Result<()> {
        self.inner.mark_exported(t, s, f)
    }
    fn verify(&self) -> Result<VerifyReport> {
        self.inner.verify()
    }
    fn reindex(&mut self) -> Result<usize> {
        self.inner.reindex()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8))]
    #[test]
    fn crash_between_eml_and_commit_resumes(batch_idx in 0usize..4, after_eml in any::<bool>()) {
        let ref_dir = tempfile::tempdir().unwrap();
        let mut ref_store = FsSqliteStore::open(ref_dir.path()).unwrap();
        run(&mut inbox(35), &mut ref_store, &opts(10)).0.unwrap();

        let dir = tempfile::tempdir().unwrap();
        let point = if after_eml { FaultPoint::AfterEmlWrite } else { FaultPoint::BeforeCommit };
        let mut store = Faulty {
            inner: FsSqliteStore::open(dir.path()).unwrap(),
            commits: 0,
            fail_at: batch_idx,
            point,
        };
        let mut src = inbox(35);
        let (r, _) = run(&mut src, &mut store, &opts(10));
        prop_assert!(r.is_err());
        // resume (fault is one-shot)
        let (r, _) = run(&mut src, &mut store, &opts(10));
        r.unwrap();
        prop_assert_eq!(norm(store.messages(ACC).unwrap()), norm(ref_store.messages(ACC).unwrap()));
        prop_assert_eq!(walk(dir.path()), walk(ref_dir.path()));
        prop_assert_eq!(store.verify().unwrap().missing.len(), 0);
        prop_assert_eq!(last_uid(&store, "INBOX"), 35);
    }
}

#[test]
fn partial_fetch_of_a_still_listed_uid_is_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    src.drop_from_fetch.insert(5);
    let (r, events) = run(&mut src, &mut store, &opts(50));
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (9, 1));
    assert!(events.iter().any(|e| matches!(e, Event::MessageFailed { uid: 5, reason, .. } if reason == "missing from fetch response")));
    assert_eq!(last_uid(&store, "INBOX"), 4);
    src.clear_failures();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!((s.added, s.failed), (1, 0));
    assert_eq!(last_uid(&store, "INBOX"), 10);
    assert_eq!(store.messages(ACC).unwrap().len(), 10);
}

#[test]
fn uid_really_gone_at_source_is_not_a_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    src.expunge_on_fetch.insert(5);
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!((s.added, s.failed), (9, 0));
    assert_eq!(last_uid(&store, "INBOX"), 10);
}

#[test]
fn reconcile_with_overlapping_renumbering_never_misapplies_flags() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(3);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    // renumber so new UIDs overlap the old range: m3->1, m1->2, m2->3, new m4->4
    src.folders.get_mut("INBOX").unwrap().messages.clear();
    src.folders.get_mut("INBOX").unwrap().uidvalidity = 99;
    src.add_message("INBOX", 1, eml(3), &["\\Seen"]);
    src.add_message("INBOX", 2, eml(1), &[]);
    src.add_message("INBOX", 3, eml(2), &[]);
    src.add_message("INBOX", 4, eml(4), &[]);
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.added, 1);
    assert_eq!(src.fetched_uids, vec![4]);
    src.set_flags("INBOX", 1, &["\\Seen", "\\Flagged"]);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    let sha = |n: u32| mailbackup_core::sha256_hex(&eml(n));
    for m in store.messages(ACC).unwrap() {
        if m.sha256 == sha(1) || m.sha256 == sha(2) {
            assert!(
                m.flags.is_empty(),
                "flags leaked onto the wrong message: {m:?}"
            );
        }
        if let Some(u) = m.uid {
            // any location that still has a uid must match the source's message at that uid
            let expected = src.folders["INBOX"]
                .messages
                .iter()
                .find(|x| x.uid == u)
                .unwrap();
            assert_eq!(m.sha256, mailbackup_core::sha256_hex(&expected.bytes));
        }
    }
}

#[test]
fn interrupted_reconcile_keeps_old_uidvalidity_and_reconciles_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(4);
    run(&mut src, &mut store, &opts(5)).0.unwrap();
    src.renumber("INBOX", 99, 100);
    for u in 105..=110 {
        src.add_message("INBOX", u, eml(u), &[]);
    }
    src.fetched_uids.clear();
    src.fail_after_fetches = Some(src.fetch_calls + 1);
    let (r, _) = run(&mut src, &mut store, &opts(5));
    assert!(matches!(r, Err(Error::Imap(_))));
    assert_eq!(
        store
            .folder_state(ACC, "INBOX")
            .unwrap()
            .unwrap()
            .uidvalidity,
        7
    );
    src.clear_failures();
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(5)).0.unwrap();
    assert_eq!(src.fetched_uids, vec![106, 107, 108, 109, 110]);
    assert_eq!(s.added, 5);
    assert_eq!(
        store.folder_state(ACC, "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 99,
            last_uid: 110
        })
    );
    let shas: std::collections::BTreeSet<_> = store
        .messages(ACC)
        .unwrap()
        .iter()
        .map(|m| m.sha256.clone())
        .collect();
    assert_eq!(shas.len(), 10);
}

#[test]
fn reconcile_with_failed_message_keeps_old_uidvalidity() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(3);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    src.renumber("INBOX", 99, 100);
    src.add_message("INBOX", 110, eml(110), &[]);
    src.add_message("INBOX", 111, eml(111), &[]);
    src.fail_uids.insert(110);
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(s.failed, 1);
    assert_eq!(
        store
            .folder_state(ACC, "INBOX")
            .unwrap()
            .unwrap()
            .uidvalidity,
        7
    );
    src.clear_failures();
    src.fetched_uids.clear();
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(src.fetched_uids, vec![110]);
    assert_eq!(
        store.folder_state(ACC, "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 99,
            last_uid: 111
        })
    );
}

#[test]
fn reconcile_always_downloads_messages_without_message_id() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("INBOX", 1);
    src.add_message("INBOX", 1, b"Subject: a\r\n\r\n".to_vec(), &[]);
    src.add_message("INBOX", 2, b"Subject: b\r\n\r\n".to_vec(), &[]);
    src.add_message("INBOX", 3, eml(3), &[]);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    src.renumber("INBOX", 2, 100);
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!(src.fetched_uids, vec![101, 102]);
    assert_eq!((s.added, s.duplicates), (0, 2));
}

#[test]
fn all_fail_batch_errors_but_keeps_earlier_batches() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    src.fail_uids.extend([5, 6, 7, 8]);
    let (r, events) = run(&mut src, &mut store, &opts(4));
    assert!(matches!(r, Err(Error::Imap(_))));
    let failed: Vec<u32> = events
        .iter()
        .filter_map(|e| {
            if let Event::MessageFailed { uid, .. } = e {
                Some(*uid)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(failed, vec![5, 6, 7, 8]);
    assert_eq!(store.messages(ACC).unwrap().len(), 4);
    assert_eq!(last_uid(&store, "INBOX"), 4);
}

// ---- final fix wave: per-folder failures (I1) ----

fn three_folders() -> FakeSource {
    let mut f = FakeSource::new();
    for (name, base) in [("A", 0u32), ("B", 10), ("INBOX", 20)] {
        f.add_folder(name, 7);
        for i in 1..=2 {
            f.add_message(name, i, eml(base + i), &[]);
        }
    }
    f
}

fn folder_failures(events: &[Event]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::FolderFailed { folder, reason } => Some((folder.clone(), reason.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn folder_that_cannot_be_selected_is_reported_and_later_folders_are_archived() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = three_folders();
    src.fail_select.insert("A".into(), Injected::Imap);
    let (r, events) = run(&mut src, &mut store, &opts(50));
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (4, 1));
    let ff = folder_failures(&events);
    assert_eq!(ff.len(), 1);
    assert_eq!(ff[0].0, "A");
    assert!(ff[0].1.contains("select"), "{ff:?}");
    assert_eq!(store.folder_state(ACC, "A").unwrap(), None);
    assert_eq!(last_uid(&store, "B"), 2);
    assert_eq!(last_uid(&store, "INBOX"), 2);
    assert!(matches!(events.last(), Some(Event::Finished(f)) if f.failed == 1));
    // once the folder is healthy again it is archived
    src.clear_failures();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!((s.added, s.failed), (2, 0));
}

#[test]
fn folder_whose_search_fails_is_reported_and_the_run_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = three_folders();
    src.fail_uids_after.insert("B".into(), Injected::Imap);
    let (r, events) = run(&mut src, &mut store, &opts(50));
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (4, 1));
    assert_eq!(
        folder_failures(&events)
            .iter()
            .map(|f| f.0.as_str())
            .collect::<Vec<_>>(),
        vec!["B"]
    );
    assert_eq!(last_uid(&store, "INBOX"), 2);
}

#[test]
fn connection_error_on_a_folder_still_aborts_the_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = three_folders();
    src.fail_select.insert("A".into(), Injected::Connection);
    let (r, _) = run(&mut src, &mut store, &opts(50));
    assert!(matches!(r, Err(Error::Connection(_))), "{r:?}");
    assert_eq!(store.folder_state(ACC, "B").unwrap(), None);
    let mut src = three_folders();
    src.fail_uids_after.insert("B".into(), Injected::Connection);
    let (r, _) = run(&mut src, &mut store, &opts(50));
    assert!(matches!(r, Err(Error::Connection(_))), "{r:?}");
    assert_eq!(store.folder_state(ACC, "INBOX").unwrap(), None);
}

// ---- folder filter (I5) ----

#[test]
fn folder_filter_matches_inbox_case_insensitively_and_reports_missing_folders() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    // As ImapSource presents a "."-delimiter server: canonical "/" separators.
    src.add_folder("INBOX", 1);
    src.add_folder("INBOX/Enviados", 1);
    src.add_folder("Outra", 1);
    src.add_message("INBOX", 1, eml(1), &[]);
    src.add_message("INBOX/Enviados", 1, eml(2), &[]);
    src.add_message("Outra", 1, eml(3), &[]);
    let o = ArchiveOptions {
        account: ACC.into(),
        folders: Some(vec![
            "inbox".into(),
            "Inbox/Enviados".into(),
            "INBOX.Enviados".into(),
            "Nope".into(),
        ]),
        since: None,
        batch_size: 50,
    };
    let (r, events) = run(&mut src, &mut store, &o);
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (2, 2));
    // stored under the server's spelling
    assert_eq!(last_uid(&store, "INBOX"), 1);
    assert_eq!(last_uid(&store, "INBOX/Enviados"), 1);
    assert_eq!(store.folder_state(ACC, "Outra").unwrap(), None);
    let ff = folder_failures(&events);
    assert_eq!(
        ff,
        vec![
            ("INBOX.Enviados".to_string(), "folder not found".to_string()),
            ("Nope".to_string(), "folder not found".to_string())
        ]
    );
}

// ---- reconcile is folder-scoped (I2) ----

#[test]
fn reconcile_skips_by_message_id_only_within_the_same_folder() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("A", 1);
    src.add_folder("B", 1);
    src.add_message("A", 1, eml(1), &[]);
    src.add_message("B", 1, eml(2), &[]);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    // B is reset; it now also holds a copy of A's message with the same Message-ID but
    // different bytes, and the very same bytes moved from A.
    let other_bytes = eml(1)
        .into_iter()
        .chain(b"extra\r\n".iter().copied())
        .collect::<Vec<u8>>();
    src.renumber("B", 2, 100);
    src.add_message("B", 102, other_bytes.clone(), &[]);
    src.add_message("B", 103, eml(1), &[]);
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    // uid 101 (m2) is already archived in B: skipped; 102 and 103 are not in B: downloaded
    assert_eq!(src.fetched_uids, vec![102, 103]);
    assert_eq!((s.added, s.duplicates), (1, 1));
    let in_b: std::collections::BTreeSet<String> = store
        .messages(ACC)
        .unwrap()
        .into_iter()
        .filter(|m| m.folder == "B")
        .map(|m| m.sha256)
        .collect();
    let sha = mailbackup_core::sha256_hex;
    assert!(in_b.contains(&sha(&other_bytes)));
    assert!(
        in_b.contains(&sha(&eml(1))),
        "moved message must get a location in B"
    );
    assert_eq!(
        store.folder_state(ACC, "B").unwrap(),
        Some(StoredFolder {
            uidvalidity: 2,
            last_uid: 103
        })
    );
}

// ---- poison message (I3) ----

#[test]
fn retry_after_a_failed_uid_fetches_only_that_uid_and_then_jumps_to_the_top() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(10);
    src.fail_uids.insert(5);
    run(&mut src, &mut store, &opts(3)).0.unwrap();
    assert_eq!(last_uid(&store, "INBOX"), 4);
    // still failing: only uid 5 is requested again, nothing above it
    src.fetched_uids.clear();
    let (r, events) = run(&mut src, &mut store, &opts(3));
    assert_eq!(r.unwrap().failed, 1);
    assert_eq!(src.fetched_uids, vec![5]);
    assert!(matches!(
        events.first(),
        Some(Event::FolderStarted { total: 1, .. })
    ));
    assert_eq!(last_uid(&store, "INBOX"), 4);
    // healthy again
    src.clear_failures();
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(3)).0.unwrap();
    assert_eq!((s.added, s.failed), (1, 0));
    assert_eq!(src.fetched_uids, vec![5]);
    assert_eq!(last_uid(&store, "INBOX"), 10);
    src.fetched_uids.clear();
    run(&mut src, &mut store, &opts(3)).0.unwrap();
    assert!(src.fetched_uids.is_empty());
}

#[test]
fn a_lone_poison_message_fails_only_itself_and_later_folders_still_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = three_folders();
    src.add_message("A", 3, eml(3), &[]);
    run(&mut src, &mut store, &opts(50)).0.unwrap();
    // a new message in A that can never be downloaded, alone in its batch
    src.add_message("A", 4, eml(4), &[]);
    src.add_message("B", 3, eml(13), &[]);
    src.fail_uids.insert(4);
    let (r, events) = run(&mut src, &mut store, &opts(50));
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (1, 1));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::MessageFailed { uid: 4, .. })));
    assert!(folder_failures(&events).is_empty());
    assert_eq!(last_uid(&store, "A"), 3);
    assert_eq!(last_uid(&store, "B"), 3);
}

// ---- backfill after the `since` filter changes (I4, with I3) ----

#[test]
fn clearing_since_after_a_failed_uid_backfills_only_missing_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("INBOX", 7);
    for i in 1..=10u32 {
        // 1..=3 are older than the filter
        src.add_message_dated(
            "INBOX",
            i,
            eml(i),
            &[],
            Some(if i <= 3 { 100 } else { 1_000 }),
        );
    }
    src.fail_uids.insert(6);
    let with_since = ArchiveOptions {
        account: ACC.into(),
        folders: None,
        since: Some(500),
        batch_size: 4,
    };
    let s = run(&mut src, &mut store, &with_since).0.unwrap();
    assert_eq!((s.added, s.failed), (6, 1));
    assert_eq!(last_uid(&store, "INBOX"), 5);

    // `profile edit --clear-since` resets the listing position, keeping UIDVALIDITY
    store.reset_last_uid(ACC).unwrap();
    src.clear_failures();
    src.fetched_uids.clear();
    let s = run(&mut src, &mut store, &opts(4)).0.unwrap();
    assert_eq!(src.fetched_uids, vec![1, 2, 3, 6]);
    assert_eq!((s.added, s.failed), (4, 0));
    assert_eq!(
        store.folder_state(ACC, "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 7,
            last_uid: 10
        })
    );
    assert_eq!(store.messages(ACC).unwrap().len(), 10);
}

// ---- reindex keeps exact folder names (I7) ----

#[test]
fn reindex_then_archive_creates_no_phantom_folders() {
    let dir = tempfile::tempdir().unwrap();
    let mut src = FakeSource::new();
    let names = ["INBOX", "Trabalho/Projetos 2024", "Álbum de Fotos", "a:b"];
    for (i, f) in names.iter().enumerate() {
        src.add_folder(f, 3);
        src.add_message(f, 1, eml(i as u32 + 1), &[]);
    }
    {
        let mut store = FsSqliteStore::open(dir.path()).unwrap();
        run(&mut src, &mut store, &opts(50)).0.unwrap();
    }
    for f in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = std::fs::remove_file(dir.path().join(f));
    }
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    store.reindex().unwrap();
    let s = run(&mut src, &mut store, &opts(50)).0.unwrap();
    assert_eq!((s.added, s.duplicates, s.failed), (0, 4, 0));
    let folders: std::collections::BTreeSet<String> = store
        .messages(ACC)
        .unwrap()
        .into_iter()
        .map(|m| m.folder)
        .collect();
    let want: std::collections::BTreeSet<String> = names.iter().map(|s| s.to_string()).collect();
    assert_eq!(folders, want);
}
