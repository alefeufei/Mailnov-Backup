use mailbackup_core::store::{FsSqliteStore, Store};
use mailbackup_core::sync::{archive, export, ArchiveOptions};
use mailbackup_core::testing::FakeSource;
use mailbackup_core::{EmlDirTarget, Error, Event, MailTarget, Result, RunSummary};
use std::collections::BTreeMap;
use std::collections::HashSet;
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, Sender};

const ACC: &str = "me";

fn eml(n: u32) -> Vec<u8> {
    format!("Message-ID: <m{n}@x>\r\nFrom: A <a@x.com>\r\nSubject: s{n}\r\n\r\nbody {n}\r\n")
        .into_bytes()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Put {
    folder: String,
    bytes: Vec<u8>,
    flags: Vec<String>,
    date: Option<i64>,
}

#[derive(Default)]
struct FakeTarget {
    ensured: Vec<String>,
    puts: Vec<Put>,
    /// `put` fails for these exact bodies with `Error::Imap`.
    fail_bytes: HashSet<Vec<u8>>,
    /// Every `put` fails with this kind of error (0 = Imap, 1 = Io).
    fail_all_with: Option<u8>,
    fail_ensure: HashSet<String>,
}

impl MailTarget for FakeTarget {
    fn id(&self) -> String {
        "fake://target".into()
    }
    fn ensure_folder(&mut self, folder: &str) -> Result<()> {
        if self.fail_ensure.contains(folder) {
            return Err(Error::Imap("cannot create".into()));
        }
        self.ensured.push(folder.to_string());
        Ok(())
    }
    fn put(
        &mut self,
        folder: &str,
        bytes: &[u8],
        flags: &[String],
        date: Option<i64>,
    ) -> Result<()> {
        match self.fail_all_with {
            Some(0) => return Err(Error::Imap("down".into())),
            Some(_) => return Err(Error::Io(std::io::Error::other("disk"))),
            None => {}
        }
        if self.fail_bytes.contains(bytes) {
            return Err(Error::Imap("rejected".into()));
        }
        self.puts.push(Put {
            folder: folder.into(),
            bytes: bytes.to_vec(),
            flags: flags.to_vec(),
            date,
        });
        Ok(())
    }
}

fn populate(src: &mut FakeSource, store: &mut FsSqliteStore) {
    let opts = ArchiveOptions {
        account: ACC.into(),
        folders: None,
        since: None,
        batch_size: 50,
    };
    let (tx, _rx) = channel();
    archive(src, store, &opts, &tx).unwrap();
}

fn source() -> FakeSource {
    let mut f = FakeSource::new();
    f.add_folder("INBOX", 7);
    f.add_message_dated(
        "INBOX",
        1,
        eml(1),
        &["\\Seen", "$Label"],
        Some(1_700_000_000),
    );
    f.add_message_dated("INBOX", 2, eml(2), &[], None);
    f.add_folder("Pai/Filho", 3);
    f.add_message_dated("Pai/Filho", 1, eml(3), &["\\Flagged"], Some(1_600_000_000));
    f
}

fn setup() -> (tempfile::TempDir, FsSqliteStore) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    populate(&mut source(), &mut store);
    (dir, store)
}

fn run(store: &mut dyn Store, target: &mut dyn MailTarget) -> (Result<RunSummary>, Vec<Event>) {
    let (tx, rx): (Sender<Event>, Receiver<Event>) = channel();
    let r = export(store, target, ACC, &tx);
    drop(tx);
    (r, rx.try_iter().collect())
}

#[test]
fn export_creates_folders_and_puts_each_message_once() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    let (r, events) = run(&mut store, &mut t);
    let s = r.unwrap();
    assert_eq!((s.added, s.duplicates, s.failed), (3, 0, 0));
    let mut ensured = t.ensured.clone();
    ensured.sort();
    assert_eq!(
        ensured,
        vec!["INBOX", "Pai/Filho"],
        "each folder ensured exactly once"
    );
    assert_eq!(t.puts.len(), 3);
    let bodies: HashSet<Vec<u8>> = t.puts.iter().map(|p| p.bytes.clone()).collect();
    assert_eq!(bodies, [eml(1), eml(2), eml(3)].into_iter().collect());
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::FolderStarted { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|e| matches!(e, Event::MessageArchived { .. }))
            .count(),
        3
    );
    assert_eq!(events.last(), Some(&Event::Finished(s)));
}

#[test]
fn second_export_sends_nothing() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    run(&mut store, &mut t).0.unwrap();
    let mut t2 = FakeTarget::default();
    let s = run(&mut store, &mut t2).0.unwrap();
    assert_eq!((s.added, s.duplicates, s.failed), (0, 3, 0));
    assert!(t2.puts.is_empty());
    assert!(t2.ensured.is_empty(), "nothing pending, nothing to ensure");
}

#[test]
fn export_state_is_per_target() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    run(&mut store, &mut t).0.unwrap();
    struct Other(FakeTarget);
    impl MailTarget for Other {
        fn id(&self) -> String {
            "fake://other".into()
        }
        fn ensure_folder(&mut self, f: &str) -> Result<()> {
            self.0.ensure_folder(f)
        }
        fn put(&mut self, f: &str, b: &[u8], fl: &[String], d: Option<i64>) -> Result<()> {
            self.0.put(f, b, fl, d)
        }
    }
    let mut o = Other(FakeTarget::default());
    assert_eq!(run(&mut store, &mut o).0.unwrap().added, 3);
}

#[test]
fn export_preserves_flags_and_internal_date() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    run(&mut store, &mut t).0.unwrap();
    let find = |b: Vec<u8>| t.puts.iter().find(|p| p.bytes == b).unwrap().clone();
    let p1 = find(eml(1));
    assert_eq!(p1.folder, "INBOX");
    assert_eq!(p1.flags, vec!["\\Seen".to_string(), "$Label".to_string()]);
    assert_eq!(p1.date, Some(1_700_000_000));
    let p2 = find(eml(2));
    assert!(p2.flags.is_empty());
    assert_eq!(p2.date, None);
    let p3 = find(eml(3));
    assert_eq!(
        (p3.folder.as_str(), p3.date),
        ("Pai/Filho", Some(1_600_000_000))
    );
    assert_eq!(p3.flags, vec!["\\Flagged".to_string()]);
}

#[test]
fn failure_on_one_message_does_not_abort_and_retries_next_run() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    t.fail_bytes.insert(eml(2));
    let (r, events) = run(&mut store, &mut t);
    let s = r.unwrap();
    assert_eq!((s.added, s.duplicates, s.failed), (2, 0, 1));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::MessageFailed { folder, uid: 2, .. } if folder == "INBOX")));
    assert_eq!(t.puts.len(), 2);

    let mut t2 = FakeTarget::default();
    let s2 = run(&mut store, &mut t2).0.unwrap();
    assert_eq!((s2.added, s2.duplicates, s2.failed), (1, 2, 0));
    assert_eq!(t2.puts.len(), 1);
    assert_eq!(t2.puts[0].bytes, eml(2));
}

#[test]
fn a_single_failing_message_alone_is_not_a_fatal_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("INBOX", 1);
    src.add_message("INBOX", 1, eml(1), &[]);
    populate(&mut src, &mut store);
    let mut t = FakeTarget::default();
    t.fail_bytes.insert(eml(1));
    let s = run(&mut store, &mut t).0.unwrap();
    assert_eq!((s.added, s.failed), (0, 1));
}

#[test]
fn every_message_failing_with_the_same_kind_of_error_aborts_the_run() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget {
        fail_all_with: Some(0),
        ..Default::default()
    };
    let (r, _) = run(&mut store, &mut t);
    assert!(matches!(r, Err(Error::Imap(_))), "{r:?}");
    // Nothing was marked, so a healthy target gets everything.
    let mut ok = FakeTarget::default();
    assert_eq!(run(&mut store, &mut ok).0.unwrap().added, 3);
}

#[test]
fn failures_of_different_kinds_do_not_abort() {
    let (_d, mut store) = setup();
    // Two Imap failures and one Io failure: not "the same kind" for every message.
    struct Mixed(FakeTarget);
    impl MailTarget for Mixed {
        fn id(&self) -> String {
            "fake://mixed".into()
        }
        fn ensure_folder(&mut self, f: &str) -> Result<()> {
            self.0.ensure_folder(f)
        }
        fn put(&mut self, _f: &str, b: &[u8], _fl: &[String], _d: Option<i64>) -> Result<()> {
            if b == eml(3).as_slice() {
                Err(Error::Io(std::io::Error::other("disk")))
            } else {
                Err(Error::Imap("no".into()))
            }
        }
    }
    let mut t = Mixed(FakeTarget::default());
    let s = run(&mut store, &mut t).0.unwrap();
    assert_eq!((s.added, s.failed), (0, 3));
}

#[test]
fn folder_that_cannot_be_ensured_fails_its_messages_and_continues() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    t.fail_ensure.insert("Pai/Filho".into());
    let (r, events) = run(&mut store, &mut t);
    let s = r.unwrap();
    assert_eq!((s.added, s.failed), (2, 1));
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::MessageFailed { folder, .. } if folder == "Pai/Filho")));
    assert!(t.puts.iter().all(|p| p.folder == "INBOX"));
}

#[test]
fn same_message_twice_in_one_folder_is_exported_once() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let mut src = FakeSource::new();
    src.add_folder("INBOX", 1);
    src.add_message("INBOX", 1, eml(1), &[]);
    src.add_message("INBOX", 2, eml(1), &[]);
    populate(&mut src, &mut store);
    let mut t = FakeTarget::default();
    let s = run(&mut store, &mut t).0.unwrap();
    assert_eq!(t.puts.len(), 1);
    assert_eq!(s.added, 1);
}

#[test]
fn closed_event_receiver_does_not_fail_the_export() {
    let (_d, mut store) = setup();
    let mut t = FakeTarget::default();
    let (tx, rx) = channel();
    drop(rx);
    assert_eq!(export(&mut store, &mut t, ACC, &tx).unwrap().added, 3);
}

fn walk(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn go(base: &Path, d: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                go(base, &p, out)
            } else {
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
    go(root, root, &mut m);
    m
}

#[test]
fn eml_target_writes_account_folder_structure_with_original_bytes() {
    let (_d, mut store) = setup();
    let out = tempfile::tempdir().unwrap();
    let mut t = EmlDirTarget::new(out.path());
    let s = run(&mut store, &mut t).0.unwrap();
    assert_eq!(s.added, 3);
    let files = walk(out.path());
    assert_eq!(files.len(), 3, "{:?}", files.keys());
    assert!(
        files
            .keys()
            .all(|k| k.ends_with(".eml") && !k.contains(".tmp")),
        "{:?}",
        files.keys()
    );
    // Nested folders become nested directories.
    let nested: Vec<&String> = files
        .keys()
        .filter(|k| k.starts_with("Pai/Filho/"))
        .collect();
    assert_eq!(nested.len(), 1, "{:?}", files.keys());
    assert_eq!(files[nested[0]], eml(3));
    let inbox: Vec<&Vec<u8>> = files
        .iter()
        .filter(|(k, _)| k.starts_with("INBOX/"))
        .map(|(_, v)| v)
        .collect();
    assert_eq!(inbox.len(), 2);
    assert!(inbox.contains(&&eml(1)) && inbox.contains(&&eml(2)));
}

#[test]
fn eml_target_sanitizes_folder_names_per_component() {
    let out = tempfile::tempdir().unwrap();
    let mut t = EmlDirTarget::new(out.path());
    t.ensure_folder("Pai:x/CON/..").unwrap();
    t.put("Pai:x/CON/..", b"hello", &[], None).unwrap();
    let files = walk(out.path());
    assert_eq!(files.len(), 1);
    let key = files.keys().next().unwrap();
    let parts: Vec<&str> = key.split('/').collect();
    assert_eq!(parts.len(), 4, "{key}");
    assert!(
        !key.contains(':') && !parts.contains(&"..") && !parts.contains(&"CON"),
        "{key}"
    );
}

#[test]
fn eml_target_rerun_does_not_duplicate_even_without_export_state() {
    let out = tempfile::tempdir().unwrap();
    let mut t = EmlDirTarget::new(out.path());
    t.put("INBOX", b"same", &["\\Seen".to_string()], Some(5))
        .unwrap();
    t.put("INBOX", b"same", &["\\Seen".to_string()], Some(5))
        .unwrap();
    t.put("INBOX", b"other", &[], Some(5)).unwrap();
    let mut t2 = EmlDirTarget::new(out.path());
    t2.put("INBOX", b"same", &[], Some(5)).unwrap();
    assert_eq!(walk(out.path()).len(), 2);
}

#[test]
fn eml_target_id_is_stable_and_names_the_directory() {
    let out = tempfile::tempdir().unwrap();
    let canonical = out.path().canonicalize().unwrap();
    let a = EmlDirTarget::new(out.path()).id();
    let b = EmlDirTarget::new(&out.path().join(".")).id();
    assert_eq!(a, b);
    assert!(a.contains(&canonical.to_string_lossy().to_string()), "{a}");
    let other = tempfile::tempdir().unwrap();
    assert_ne!(a, EmlDirTarget::new(other.path()).id());
}
