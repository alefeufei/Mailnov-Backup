use mailbackup_core::store::{BatchOutcome, FaultPoint, FsSqliteStore, Store, StoredFolder, StoredMessage, VerifyReport};
use mailbackup_core::sync::{archive, ArchiveOptions};
use mailbackup_core::testing::FakeSource;
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
    ArchiveOptions { account: ACC.into(), folders: None, since: None, batch_size: batch }
}

fn inbox(n: u32) -> FakeSource {
    let mut f = FakeSource::new();
    f.add_folder("INBOX", 7);
    for i in 1..=n {
        f.add_message("INBOX", i, eml(i), &[]);
    }
    f
}

fn run(src: &mut FakeSource, store: &mut dyn Store, o: &ArchiveOptions) -> (Result<RunSummary>, Vec<Event>) {
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
                out.insert(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap());
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
    assert!(matches!(events.first(), Some(Event::FolderStarted { total: 120, .. })));
    assert!(matches!(events.last(), Some(Event::Finished(_))));
    assert_eq!(events.iter().filter(|e| matches!(e, Event::MessageArchived { .. })).count(), 120);
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
    assert_eq!(norm(store.messages(ACC).unwrap()), norm(ref_store.messages(ACC).unwrap()));
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
        Some(StoredFolder { uidvalidity: 99, last_uid: 200 })
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
        Some(StoredFolder { uidvalidity: 99, last_uid: 105 })
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
    assert!(events.iter().any(|e| matches!(e, Event::MessageFailed { uid: 5, .. })));
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
    assert_eq!(store.verify().unwrap(), VerifyReport { checked: 5, missing: vec![], corrupt: vec![] });
}

#[test]
fn flags_are_updated_for_known_messages() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = inbox(3);
    run(&mut src, &mut store, &opts(2)).0.unwrap();
    assert!(store.messages(ACC).unwrap().iter().all(|m| m.flags.is_empty()));
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
    let o = ArchiveOptions { account: ACC.into(), folders: Some(vec!["INBOX".into()]), since: Some(150), batch_size: 50 };
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
    fn commit_batch(&mut self, a: &str, f: &str, v: u32, b: &[RawMessage], l: u32) -> Result<BatchOutcome> {
        if self.commits == self.fail_at {
            self.inner.inject_fault(self.point);
        }
        self.commits += 1;
        self.inner.commit_batch(a, f, v, b, l)
    }
    fn has_message_id(&self, a: &str, m: &str) -> Result<bool> {
        self.inner.has_message_id(a, m)
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
