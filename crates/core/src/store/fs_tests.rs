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

#[test]
fn reset_last_uid_zeroes_only_that_account_and_keeps_uidvalidity() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 7, &[raw(5, 1)], 5).unwrap();
    s.commit_batch("me", "Sent", 8, &[raw(3, 2)], 3).unwrap();
    s.commit_batch("other", "INBOX", 9, &[raw(4, 3)], 4)
        .unwrap();
    s.reset_last_uid("me").unwrap();
    assert_eq!(
        s.folder_state("me", "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 7,
            last_uid: 0
        })
    );
    assert_eq!(
        s.folder_state("me", "Sent").unwrap(),
        Some(StoredFolder {
            uidvalidity: 8,
            last_uid: 0
        })
    );
    assert_eq!(
        s.folder_state("other", "INBOX").unwrap(),
        Some(StoredFolder {
            uidvalidity: 9,
            last_uid: 4
        })
    );
    assert_eq!(s.messages("me").unwrap().len(), 2);
    // unknown account: nothing to do
    s.reset_last_uid("nobody").unwrap();
}

#[test]
fn read_eml_reports_missing_and_tampered_files_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    s.commit_batch("me", "INBOX", 1, &[raw(1, 1), raw(2, 2)], 2)
        .unwrap();
    let sha1 = sha256_hex(&eml(1));
    let sha2 = sha256_hex(&eml(2));
    std::fs::remove_file(dir.path().join(eml_relpath("me", "INBOX", &sha1))).unwrap();
    let mut flipped = eml(2);
    flipped[0] ^= 0x01;
    std::fs::write(dir.path().join(eml_relpath("me", "INBOX", &sha2)), flipped).unwrap();
    match s.read_eml(&sha1) {
        Err(Error::Corrupt(m)) => assert!(m.contains(&sha1), "{m}"),
        other => panic!("{other:?}"),
    }
    match s.read_eml(&sha2) {
        Err(Error::Corrupt(m)) => assert!(m.contains(&sha2), "{m}"),
        other => panic!("{other:?}"),
    }
}

fn wipe_index(root: &Path) {
    for f in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = std::fs::remove_file(root.join(f));
    }
}

const ODD_FOLDERS: [&str; 5] = [
    "INBOX",
    "Trabalho/Projetos 2024",
    "Álbum de Fotos",
    "a:b?c",
    "Pai/Filho/Neto. ",
];

#[test]
fn reindex_recovers_exact_folder_and_account_names_from_sidecars() {
    let dir = tempfile::tempdir().unwrap();
    let account = "conta:1/x";
    {
        let mut s = FsSqliteStore::open(dir.path()).unwrap();
        for (i, f) in ODD_FOLDERS.iter().enumerate() {
            s.commit_batch(account, f, 1, &[raw(1, i as u32 + 1)], 1)
                .unwrap();
        }
        // a duplicate in another folder writes no file there
        s.commit_batch(account, "Dup", 1, &[raw(9, 1)], 9).unwrap();
    }
    wipe_index(dir.path());
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    assert_eq!(s.reindex().unwrap(), ODD_FOLDERS.len());
    let mut got: Vec<String> = s
        .messages(account)
        .unwrap()
        .into_iter()
        .map(|m| m.folder)
        .collect();
    got.sort();
    let mut want: Vec<String> = ODD_FOLDERS.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want);
    // the sidecars are never mistaken for messages
    assert_eq!(s.verify().unwrap().checked, ODD_FOLDERS.len());
}

#[test]
fn reindex_of_a_legacy_archive_without_sidecars_uses_directory_names() {
    let dir = tempfile::tempdir().unwrap();
    {
        let mut s = FsSqliteStore::open(dir.path()).unwrap();
        s.commit_batch("me", "INBOX", 1, &[raw(1, 1)], 1).unwrap();
        s.commit_batch("me", "a:b", 1, &[raw(1, 2)], 1).unwrap();
    }
    wipe_index(dir.path());
    fn strip(d: &Path) {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                strip(&p);
            } else if p
                .file_name()
                .is_some_and(|n| n == ".folder" || n == ".account")
            {
                std::fs::remove_file(p).unwrap();
            }
        }
    }
    strip(&dir.path().join("mail"));
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    assert_eq!(s.reindex().unwrap(), 2);
    let mut got: Vec<String> = s
        .messages("me")
        .unwrap()
        .into_iter()
        .map(|m| m.folder)
        .collect();
    got.sort();
    assert_eq!(
        got,
        vec![
            "INBOX".to_string(),
            crate::store::paths::safe_component("a:b")
        ]
    );
}

#[test]
fn folders_named_like_sidecars_archive_and_reindex_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let names = [
        ".account",
        ".folder",
        ".account.tmp",
        ".folder.tmp",
        ".Trash",
    ];
    {
        let mut s = FsSqliteStore::open(dir.path()).unwrap();
        // the account sidecar exists before any of these folders is created
        s.commit_batch("me", "INBOX", 1, &[raw(1, 99)], 1).unwrap();
        for (i, f) in names.iter().enumerate() {
            s.commit_batch("me", f, 1, &[raw(1, i as u32 + 1)], 1)
                .unwrap();
        }
        assert_eq!(s.verify().unwrap().checked, names.len() + 1);
    }
    wipe_index(dir.path());
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    assert_eq!(s.reindex().unwrap(), names.len() + 1);
    let mut got: Vec<String> = s
        .messages("me")
        .unwrap()
        .into_iter()
        .map(|m| m.folder)
        .collect();
    got.sort();
    let mut want: Vec<String> = names.iter().map(|n| n.to_string()).collect();
    want.push("INBOX".into());
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn reindex_keeps_legacy_dot_directories() {
    // Written before folder names starting with "." were escaped (and before sidecars).
    let dir = tempfile::tempdir().unwrap();
    FsSqliteStore::open(dir.path()).unwrap();
    let sha = sha256_hex(&eml(1));
    let legacy = dir
        .path()
        .join("mail")
        .join("me")
        .join(".Trash")
        .join(&sha[..2]);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(legacy.join(format!("{sha}.eml")), eml(1)).unwrap();
    wipe_index(dir.path());
    let mut s = FsSqliteStore::open(dir.path()).unwrap();
    assert_eq!(s.reindex().unwrap(), 1);
    assert_eq!(s.messages("me").unwrap()[0].folder, ".Trash");
    assert_eq!(s.read_eml(&sha).unwrap(), eml(1));
}
