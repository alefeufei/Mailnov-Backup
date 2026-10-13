use super::*;
use crate::store::paths::eml_relpath;
use crate::{sha256_hex, RawMessage};
use std::path::Path;

fn eml(n: u32) -> Vec<u8> {
    format!(
        "Message-ID: <m{n}@x>\r\nFrom: A <a@x.com>\r\nTo: b@x.com\r\nSubject: s{n}\r\nDate: Mon, 02 Jan 2006 15:04:05 +0000\r\n\r\nbody {n}\r\n"
    )
    .into_bytes()
}

fn raw(uid: u32, n: u32) -> RawMessage {
    RawMessage {
        uid,
        flags: vec!["\\Seen".into()],
        internal_date: Some(1000),
        bytes: eml(n),
    }
}

fn walk_eml(root: &Path) -> Vec<std::path::PathBuf> {
    fn go(d: &Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd {
            let p = e.unwrap().path();
            if p.is_dir() {
                go(&p, out)
            } else if p.extension().is_some_and(|x| x == "eml") {
                out.push(p)
            }
        }
    }
    let mut v = vec![];
    go(&root.join("mail"), &mut v);
    v
}

#[test]
fn commit_batch_writes_eml_and_index() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    let out = s
        .commit_batch("me", "INBOX", 7, &[raw(1, 1), raw(2, 2)], 2)
        .unwrap();
    assert_eq!(
        out,
        BatchOutcome {
            added: 2,
            duplicates: 0
        }
    );
    let sha = sha256_hex(&eml(1));
    assert!(dir.path().join(eml_relpath("me", "INBOX", &sha)).exists());
    assert_eq!(
        s.folder_state("me", "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 7,
            last_uid: 2
        })
    );
    assert_eq!(s.folder_state("me", "Nope").unwrap(), None);
    assert!(s.has_message_id("me", "INBOX", "m1@x").unwrap());
    assert!(!s.has_message_id("me", "INBOX", "zzz@x").unwrap());
    assert_eq!(s.read_eml(&sha).unwrap(), eml(1));
}

#[test]
fn same_bytes_in_two_folders_is_one_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
    let out = s.commit_batch("me", "Sent", 1, &[raw(5, 1)], 5).unwrap();
    assert_eq!(
        out,
        BatchOutcome {
            added: 0,
            duplicates: 1
        }
    );
    assert_eq!(walk_eml(dir.path()).len(), 1);
    let msgs = s.messages("me").unwrap();
    assert_eq!(msgs.len(), 2);
    let folders: Vec<_> = msgs.iter().map(|m| m.folder.as_str()).collect();
    assert!(folders.contains(&"INBOX") && folders.contains(&"Sent"));
}

#[test]
fn has_message_id_is_scoped_to_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
    s.commit_batch("me", "Sent", 1, &[raw(1, 2)], 1).unwrap();
    assert!(s.has_message_id("me", "INBOX", "m1@x").unwrap());
    assert!(!s.has_message_id("me", "Sent", "m1@x").unwrap());
    assert!(!s.has_message_id("me", "Nope", "m1@x").unwrap());
    assert!(!s.has_message_id("other", "INBOX", "m1@x").unwrap());
    // a location detached from its uid by a UIDVALIDITY change still counts
    s.commit_batch("me", "INBOX", 2, &[], 0).unwrap();
    assert!(s
        .messages("me")
        .unwrap()
        .iter()
        .any(|m| m.folder == "INBOX" && m.uid.is_none()));
    assert!(s.has_message_id("me", "INBOX", "m1@x").unwrap());
}

#[test]
fn commit_batch_is_atomic_on_db_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
    for point in [FaultPoint::AfterEmlWrite, FaultPoint::BeforeCommit] {
        s.inject_fault(point);
        let err = s.commit_batch("me", "INBOX", 1, &[raw(2, 2), raw(3, 3)], 3);
        assert!(matches!(err, Err(Error::Db(_))));
        assert_eq!(
            s.folder_state("me", "INBOX").unwrap(),
            Some(StoredFolder {
                uidvalidity: 1,
                last_uid: 1
            })
        );
        assert_eq!(s.messages("me").unwrap().len(), 1);
    }
    // retry succeeds and reuses the orphan files
    let out = s
        .commit_batch("me", "INBOX", 1, &[raw(2, 2), raw(3, 3)], 3)
        .unwrap();
    assert_eq!(out.added, 2);
    assert_eq!(s.folder_state("me", "INBOX").unwrap().unwrap().last_uid, 3);
    assert_eq!(walk_eml(dir.path()).len(), 3);
}

#[test]
fn orphan_eml_from_crash_is_reused() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    let sha = sha256_hex(&eml(1));
    let p = dir.path().join(eml_relpath("me", "INBOX", &sha));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, eml(1)).unwrap();
    let out = s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
    assert_eq!(out.added, 1);
    assert_eq!(walk_eml(dir.path()).len(), 1);
    assert_eq!(
        s.verify().unwrap(),
        VerifyReport {
            checked: 1,
            missing: vec![],
            corrupt: vec![]
        }
    );
}

#[test]
fn reopen_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut s = FsSqliteStore::open(dir.path()).unwrap();
        s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
    }
    let s = FsSqliteStore::open(dir.path()).unwrap();
    assert_eq!(s.messages("me").unwrap().len(), 1);
    assert_eq!(s.user_version().unwrap(), 1);
    assert!(dir.path().join("archive.toml").exists());
}

#[test]
fn verify_reports_missing_and_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 1, &[raw(1, 1), raw(2, 2), raw(3, 3)], 3)
        .unwrap();
    let p1 = dir
        .path()
        .join(eml_relpath("me", "INBOX", &sha256_hex(&eml(1))));
    let p2 = dir
        .path()
        .join(eml_relpath("me", "INBOX", &sha256_hex(&eml(2))));
    std::fs::remove_file(p1).unwrap();
    std::fs::write(p2, b"tampered").unwrap();
    let r = s.verify().unwrap();
    assert_eq!((r.checked, r.missing.len(), r.corrupt.len()), (3, 1, 1));
}

#[test]
fn reindex_rebuilds_message_rows() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut s = FsSqliteStore::open(dir.path()).unwrap();
        s.commit_batch("me", "INBOX", 1, &[raw(1, 1), raw(2, 2)], 2)
            .unwrap();
    }
    for f in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = std::fs::remove_file(dir.path().join(f));
    }
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    assert_eq!(s.messages("me").unwrap().len(), 0);
    assert_eq!(s.reindex().unwrap(), 2);
    let msgs = s.messages("me").unwrap();
    assert_eq!(msgs.len(), 2);
    assert!(msgs.iter().all(|m| m.uid.is_none() && m.folder == "INBOX"));
    assert!(s.has_message_id("me", "INBOX", "m1@x").unwrap());
}

#[test]
fn unparseable_and_headerless_messages_are_archived() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    let junk = RawMessage {
        uid: 1,
        flags: vec![],
        internal_date: None,
        bytes: vec![0xff, 0xfe, 0, 1],
    };
    let empty = RawMessage {
        uid: 2,
        flags: vec![],
        internal_date: None,
        bytes: b"X-Foo: bar\r\n\r\n".to_vec(),
    };
    let out = s.commit_batch("me", "INBOX", 1, &[junk, empty], 2).unwrap();
    assert_eq!(out.added, 2);
}

#[test]
fn flags_update_and_export_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
    s.update_flags("me", "INBOX", 1, &["\\Seen".into(), "\\Flagged".into()])
        .unwrap();
    assert_eq!(
        s.messages("me").unwrap()[0].flags,
        vec!["\\Seen", "\\Flagged"]
    );
    let sha = sha256_hex(&eml(1));
    assert!(!s.export_done("t", &sha, "INBOX").unwrap());
    s.mark_exported("t", &sha, "INBOX").unwrap();
    s.mark_exported("t", &sha, "INBOX").unwrap();
    assert!(s.export_done("t", &sha, "INBOX").unwrap());
}

#[test]
fn uid_collision_after_uidvalidity_change_keeps_old_location() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 10, &[raw(1, 1)], 1).unwrap();
    s.commit_batch("me", "INBOX", 11, &[raw(1, 2)], 1).unwrap();
    let msgs = s.messages("me").unwrap();
    let shas: Vec<_> = msgs.iter().map(|m| m.sha256.clone()).collect();
    assert_eq!(msgs.len(), 2);
    assert!(shas.contains(&sha256_hex(&eml(1))) && shas.contains(&sha256_hex(&eml(2))));
    assert_eq!(walk_eml(dir.path()).len(), 2);
    assert!(s.has_message_id("me", "INBOX", "m1@x").unwrap());
    // same uid, different message, same uidvalidity: old row kept with uid NULL
    s.commit_batch("me", "INBOX", 11, &[raw(1, 3)], 1).unwrap();
    assert_eq!(s.messages("me").unwrap().len(), 3);
}

#[test]
fn retry_of_same_batch_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    let batch = [raw(1, 1), raw(2, 2)];
    s.commit_batch("me", "INBOX", 1, &batch, 2).unwrap();
    s.commit_batch("me", "INBOX", 1, &batch, 2).unwrap();
    assert_eq!(s.messages("me").unwrap().len(), 2);
}
