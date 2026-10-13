//! End-to-end against a real Dovecot (see tests/dovecot/README.md). Run with:
//! `cargo test -p mailbackup-core --features testing -- --ignored archive_from_dovecot`
use mailbackup_core::store::{FsSqliteStore, Store};
use mailbackup_core::sync::{archive, ArchiveOptions};
use mailbackup_core::{sha256_hex, Event, ImapSource, MailSource, PasswordAuth, TlsMode};
use std::path::PathBuf;
use std::sync::mpsc::channel;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// The seed messages exactly as Dovecot stores/serves them (CRLF line endings).
fn seed_messages() -> Vec<Vec<u8>> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/dovecot/seed");
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "eml"))
        .collect();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(p).unwrap().replace("\r\n", "\n").replace('\n', "\r\n");
            text.into_bytes()
        })
        .collect()
}

#[test]
#[ignore = "requires `docker compose up` in tests/dovecot (see README there)"]
fn archive_from_dovecot() {
    let host = env_or("MAILBACKUP_TEST_IMAP_HOST", "localhost");
    let port: u16 = env_or("MAILBACKUP_TEST_IMAP_PORT", "993").parse().unwrap();
    let auth = PasswordAuth::new(
        env_or("MAILBACKUP_TEST_IMAP_USER", "testuser"),
        env_or("MAILBACKUP_TEST_IMAP_PASSWORD", "testpass"),
    );
    let mut source = ImapSource::connect(&host, port, TlsMode::Implicit, &auth).unwrap();

    let folders = source.list_folders().unwrap();
    assert!(folders.iter().any(|f| f == "INBOX"), "folders: {folders:?}");

    let dir = tempfile::tempdir().unwrap();
    let mut store = FsSqliteStore::open(dir.path()).unwrap();
    let opts = ArchiveOptions { account: "dovecot".into(), folders: None, since: None, batch_size: 50 };
    let (tx, rx) = channel::<Event>();
    let summary = archive(&mut source, &mut store, &opts, &tx).unwrap();
    drop(tx);
    let _events: Vec<Event> = rx.try_iter().collect();

    let seeds = seed_messages();
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.added, seeds.len(), "message count");

    let stored = store.messages("dovecot").unwrap();
    for seed in &seeds {
        let hash = sha256_hex(seed);
        assert!(stored.iter().any(|m| m.sha256 == hash), "seed with hash {hash} not archived");
        assert_eq!(&store.read_eml(&hash).unwrap(), seed);
    }

    // Second run is incremental: nothing new.
    let (tx, _rx) = channel::<Event>();
    let again = archive(&mut source, &mut store, &opts, &tx).unwrap();
    assert_eq!(again.added, 0);
}
