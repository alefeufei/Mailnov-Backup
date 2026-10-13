//! Orchestration shared by the command line and the desktop app: builds sources and targets from
//! a [`Profile`] and its stored secrets, guards against using an archive that does not exist,
//! runs the engines and returns typed results. Nothing here prints or decides exit codes.
use crate::cancel::{CancellableSource, CancellableTarget};
use crate::profile::validate_name;
use crate::store::{FsSqliteStore, Store, VerifyReport};
use crate::sync::{archive, export, ArchiveOptions};
use crate::{
    secret_key, EmlDirTarget, Endpoint, Error, Event, ImapSource, ImapTarget, MailSource,
    MailTarget, PasswordAuth, Profile, ProfileStore, Result, RunSummary, SecretRole, SecretStore,
    TargetSpec,
};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Sender;
use std::sync::Arc;

/// A message (`uid` is `Some`) or a whole folder (`uid` is `None`) that could not be processed.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Failure {
    pub folder: String,
    pub uid: Option<u32>,
    pub reason: String,
}

impl Failure {
    /// The failure an engine event reports, if any.
    pub fn from_event(e: &Event) -> Option<Failure> {
        match e {
            Event::MessageFailed {
                folder,
                uid,
                reason,
            } => Some(Failure {
                folder: folder.clone(),
                uid: Some(*uid),
                reason: reason.clone(),
            }),
            Event::FolderFailed { folder, reason } => Some(Failure {
                folder: folder.clone(),
                uid: None,
                reason: reason.clone(),
            }),
            _ => None,
        }
    }
}

/// How a finished run went: everything worked, it finished with failed items, or it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ExitClass {
    Ok,
    Partial,
    Error,
}

impl ExitClass {
    pub fn of(result: &Result<RunSummary>) -> ExitClass {
        match result {
            Ok(s) if s.failed > 0 => ExitClass::Partial,
            Ok(_) => ExitClass::Ok,
            Err(_) => ExitClass::Error,
        }
    }
}

/// Kind of export destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    Imap,
    Eml,
}

// ---- secrets ----

/// The stored password of `profile` for `role` (an `Error::Auth` naming the key when missing).
pub fn password(secrets: &dyn SecretStore, profile: &str, role: SecretRole) -> Result<String> {
    secrets.get(&secret_key(profile, role))
}

// ---- archive directory ----

/// Whether `dir` holds an archive (its `archive.toml` exists).
pub fn archive_exists(dir: &Path) -> bool {
    dir.join("archive.toml").is_file()
}

/// Opens an archive that must already exist (only archiving may create one).
/// `archive.toml` is written when the store is created and survives deleting `index.db`.
/// With `need_index`, a missing `index.db` is an error (opening would silently create an empty
/// index and make the archive look empty); only `reindex` passes `false`.
pub fn open_existing(p: &Profile, need_index: bool) -> Result<FsSqliteStore> {
    if !archive_exists(&p.archive_dir) {
        return Err(Error::Config(format!(
            "no archive found at '{}'; run `archive {}` first or check the profile's archive_dir",
            p.archive_dir.display(),
            p.name
        )));
    }
    if need_index && !p.archive_dir.join("index.db").is_file() {
        return Err(Error::Config(format!(
            "the index '{}' is missing; run `mailbackup reindex {}` to rebuild it from the .eml files",
            p.archive_dir.join("index.db").display(),
            p.name
        )));
    }
    FsSqliteStore::open(&p.archive_dir)
}

/// Opens (creating it if needed) the archive to archive into, after validating the profile's
/// `since` date.
pub fn open_for_archive(p: &Profile) -> Result<FsSqliteStore> {
    p.since_epoch()?;
    FsSqliteStore::open(&p.archive_dir)
}

/// Validates the export target (exists, kind matches `want`, password available) without
/// connecting or touching the archive.
pub fn check_target(
    p: &Profile,
    secrets: &dyn SecretStore,
    want: Option<TargetKind>,
) -> Result<()> {
    match (&p.target, want) {
        (None, _) => Err(Error::Config(format!(
            "profile '{0}' has no export target; set one with `profile edit {0} --target-eml <dir>` \
             or --target-imap-host/--target-imap-user",
            p.name
        ))),
        (Some(TargetSpec::EmlDir(_)), None | Some(TargetKind::Eml)) => Ok(()),
        (Some(TargetSpec::Imap(_)), None | Some(TargetKind::Imap)) => {
            password(secrets, &p.name, SecretRole::Target).map(|_| ())
        }
        (Some(t), Some(_)) => {
            let have = if matches!(t, TargetSpec::Imap(_)) {
                "imap"
            } else {
                "eml"
            };
            Err(Error::Config(format!(
                "profile '{}' has a {have} target, which does not match --to",
                p.name
            )))
        }
    }
}

// ---- connections ----

/// Connects (TLS always validated) and logs in to `ep`.
pub fn connect_source(ep: &Endpoint, password: String) -> Result<ImapSource> {
    ImapSource::connect(
        &ep.host,
        ep.port,
        ep.tls,
        Box::new(PasswordAuth::new(&ep.user, password)),
    )
}

/// The profile's source account, connected with its stored password.
pub fn source_for(p: &Profile, secrets: &dyn SecretStore) -> Result<ImapSource> {
    let pw = password(secrets, &p.name, SecretRole::Source)?;
    connect_source(&p.source, pw)
}

/// The profile's export target (connected when it is IMAP). Call [`check_target`] first for a
/// clear error when the profile has none.
pub fn target_for(p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailTarget>> {
    match &p.target {
        Some(TargetSpec::EmlDir(d)) => Ok(Box::new(EmlDirTarget::new(d))),
        Some(TargetSpec::Imap(e)) => {
            let pw = password(secrets, &p.name, SecretRole::Target)?;
            Ok(Box::new(ImapTarget::connect(
                &e.host,
                e.port,
                e.tls,
                Box::new(PasswordAuth::new(&e.user, pw)),
            )?))
        }
        None => Err(Error::Config(format!(
            "profile '{}' has no export target",
            p.name
        ))),
    }
}

/// Connect + LOGIN + LIST: the folder names (decoded, "/"-separated) or the error that stopped
/// the attempt. Never stores anything.
pub fn test_connection(ep: &Endpoint, password: String) -> Result<Vec<String>> {
    if ep.host.trim().is_empty() {
        return Err(Error::Config("the server name is empty".into()));
    }
    if ep.user.trim().is_empty() {
        return Err(Error::Config("the user name is empty".into()));
    }
    let mut src = connect_source(ep, password)?;
    src.list_folders()
}

/// The folders of the profile's source account.
pub fn list_remote_folders(p: &Profile, secrets: &dyn SecretStore) -> Result<Vec<String>> {
    source_for(p, secrets)?.list_folders()
}

// ---- engines ----

fn archive_options(p: &Profile) -> Result<ArchiveOptions> {
    Ok(ArchiveOptions {
        account: p.name.clone(),
        folders: p.folders.clone(),
        since: p.since_epoch()?,
        batch_size: 0,
    })
}

/// Archives `p` from an already connected `source` (cancellable when `cancel` is given).
pub fn archive_with(
    p: &Profile,
    source: &mut dyn MailSource,
    store: &mut dyn Store,
    cancel: Option<Arc<AtomicBool>>,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    let opts = archive_options(p)?;
    match cancel {
        Some(flag) => archive(
            &mut CancellableSource::new(source, flag),
            store,
            &opts,
            events,
        ),
        None => archive(source, store, &opts, events),
    }
}

/// Connects to the profile's source and archives it into `store`.
pub fn run_archive(
    p: &Profile,
    secrets: &dyn SecretStore,
    store: &mut dyn Store,
    cancel: Option<Arc<AtomicBool>>,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    archive_options(p)?;
    let mut source = source_for(p, secrets)?;
    archive_with(p, &mut source, store, cancel, events)
}

/// Exports `p`'s archive to an already built `target` (cancellable when `cancel` is given).
pub fn export_with(
    p: &Profile,
    store: &mut dyn Store,
    target: Box<dyn MailTarget>,
    cancel: Option<Arc<AtomicBool>>,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    match cancel {
        Some(flag) => export(
            store,
            &mut CancellableTarget::new(target, flag),
            &p.name,
            events,
        ),
        None => {
            let mut target = target;
            export(store, &mut target, &p.name, events)
        }
    }
}

/// Validates and builds the profile's target and exports the archive in `store` to it.
pub fn run_export(
    p: &Profile,
    secrets: &dyn SecretStore,
    store: &mut dyn Store,
    want: Option<TargetKind>,
    cancel: Option<Arc<AtomicBool>>,
    events: &Sender<Event>,
) -> Result<RunSummary> {
    check_target(p, secrets, want)?;
    let target = target_for(p, secrets)?;
    export_with(p, store, target, cancel, events)
}

/// Message count of one archived folder.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct FolderCount {
    pub folder: String,
    pub messages: usize,
}

/// Per-folder counts of an archive. `total` counts a message once per folder it is in;
/// `unique` counts distinct messages.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct StatusReport {
    pub folders: Vec<FolderCount>,
    pub total: usize,
    pub unique: usize,
}

/// Counts what `store` holds for `account`.
pub fn status_of(store: &dyn Store, account: &str) -> Result<StatusReport> {
    let mut per: BTreeMap<String, HashSet<String>> = BTreeMap::new();
    let mut unique: HashSet<String> = HashSet::new();
    for m in store.messages(account)? {
        unique.insert(m.sha256.clone());
        per.entry(m.folder).or_default().insert(m.sha256);
    }
    let folders: Vec<FolderCount> = per
        .into_iter()
        .map(|(folder, set)| FolderCount {
            folder,
            messages: set.len(),
        })
        .collect();
    Ok(StatusReport {
        total: folders.iter().map(|f| f.messages).sum(),
        unique: unique.len(),
        folders,
    })
}

/// Per-folder counts of the profile's (existing) archive.
pub fn status(p: &Profile) -> Result<StatusReport> {
    status_of(&open_existing(p, true)?, &p.name)
}

/// Checks the archive files against the index.
pub fn verify(p: &Profile) -> Result<VerifyReport> {
    open_existing(p, true)?.verify()
}

/// Rebuilds the index from the `.eml` files; returns the number of messages indexed.
pub fn reindex(p: &Profile) -> Result<usize> {
    open_existing(p, false)?.reindex()
}

// ---- profiles ----

/// Checks a profile that is about to be created: valid unused name (also ignoring case), valid
/// `since`. Touches nothing.
pub fn validate_new_profile(profiles: &ProfileStore, p: &Profile) -> Result<()> {
    validate_name(&p.name)?;
    if profiles.list()?.contains(&p.name) {
        return Err(Error::Config(format!(
            "profile '{}' already exists; use `profile edit`",
            p.name
        )));
    }
    if let Some(other) = profiles.case_conflict(&p.name)? {
        return Err(Error::Config(format!(
            "profile '{other}' already exists (profile names are case-insensitive); \
             use `profile edit {other}` or another name"
        )));
    }
    p.since_epoch()?;
    Ok(())
}

/// Makes the profile's paths absolute (without requiring them to exist), so running from
/// another working directory never silently uses a second archive.
pub fn absolutize(p: &mut Profile) -> Result<()> {
    p.archive_dir = std::path::absolute(&p.archive_dir)?;
    if let Some(TargetSpec::EmlDir(d)) = &mut p.target {
        *d = std::path::absolute(&*d)?;
    }
    Ok(())
}

/// Creates a profile: stores the source password (and the target password when given) in
/// `secrets`, then the profile. On failure nothing is left behind.
pub fn create_profile(
    profiles: &ProfileStore,
    secrets: &dyn SecretStore,
    p: &Profile,
    source_password: &str,
    target_password: Option<&str>,
) -> Result<()> {
    validate_new_profile(profiles, p)?;
    if source_password.is_empty() {
        return Err(Error::Config("empty password; nothing was stored".into()));
    }
    if matches!(p.target, Some(TargetSpec::Imap(_))) && target_password.is_none_or(str::is_empty) {
        return Err(Error::Config(
            "the IMAP target needs a password; nothing was stored".into(),
        ));
    }
    let src_key = secret_key(&p.name, SecretRole::Source);
    secrets.set(&src_key, source_password)?;
    let tgt_key = secret_key(&p.name, SecretRole::Target);
    let stored = (|| -> Result<()> {
        if let (Some(TargetSpec::Imap(_)), Some(pw)) = (&p.target, target_password) {
            secrets.set(&tgt_key, pw)?;
        }
        profiles.save(p)
    })();
    if let Err(e) = stored {
        let _ = secrets.delete(&src_key);
        let _ = secrets.delete(&tgt_key);
        return Err(e);
    }
    Ok(())
}

/// After the `since` filter changed: make the existing archive (if any; never create one) list
/// every folder from the start again, so mail outside the old filter is backfilled.
pub fn reset_archive_position(p: &Profile) -> Result<bool> {
    let dir = &p.archive_dir;
    if !archive_exists(dir) || !dir.join("index.db").is_file() {
        return Ok(false);
    }
    FsSqliteStore::open(dir)?.reset_last_uid(&p.name)?;
    Ok(true)
}

/// Saves an edited profile (it must exist). When `since` changed, the existing archive is
/// rewound first (see [`reset_archive_position`]); returns whether that happened.
pub fn update_profile(profiles: &ProfileStore, p: &Profile) -> Result<bool> {
    let old = profiles.load(&p.name)?;
    p.since_epoch()?;
    // Reset before saving: if saving then fails, the reset only costs a re-listing (UIDs
    // already archived are never downloaded again).
    let reset = p.since != old.since && reset_archive_position(p)?;
    profiles.save(p)?;
    Ok(reset)
}

/// Deletes both secrets first (idempotent), then the profile file, so a failing credential store
/// never leaves orphaned credentials behind a vanished profile. The archive is never touched.
pub fn remove_profile(
    profiles: &ProfileStore,
    secrets: &dyn SecretStore,
    name: &str,
) -> Result<()> {
    validate_name(name)?;
    if !profiles.list()?.iter().any(|n| n == name) {
        return Err(Error::Config(format!("profile '{name}' does not exist")));
    }
    secrets.delete(&secret_key(name, SecretRole::Source))?;
    secrets.delete(&secret_key(name, SecretRole::Target))?;
    profiles.remove(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cancel::is_cancelled;
    use crate::testing::FakeSource;
    use crate::{MemorySecrets, TlsMode};
    use std::sync::atomic::Ordering;
    use std::sync::mpsc::channel;

    fn ep() -> Endpoint {
        Endpoint {
            host: "imap.example.com".into(),
            port: 993,
            tls: TlsMode::Implicit,
            user: "u@example.com".into(),
        }
    }

    fn profile(name: &str, dir: &Path) -> Profile {
        Profile {
            name: name.into(),
            archive_dir: dir.join("archive"),
            source: ep(),
            folders: None,
            since: None,
            target: None,
        }
    }

    fn msg(n: u32) -> Vec<u8> {
        format!("Message-ID: <m{n}@x>\r\nSubject: s{n}\r\n\r\nbody {n}\r\n").into_bytes()
    }

    fn fake() -> FakeSource {
        let mut s = FakeSource::new();
        s.add_folder("INBOX", 7);
        s.add_folder("Sent", 7);
        for n in 1..=3 {
            s.add_message("INBOX", n, msg(n), &["\\Seen"]);
        }
        s.add_message("Sent", 1, msg(10), &[]);
        s
    }

    fn archived(d: &Path) -> (Profile, FsSqliteStore) {
        let p = profile("w", d);
        let mut store = open_for_archive(&p).unwrap();
        let (tx, _rx) = channel();
        let s = archive_with(&p, &mut fake(), &mut store, None, &tx).unwrap();
        assert_eq!((s.added, s.failed), (4, 0));
        (p, store)
    }

    #[test]
    fn open_existing_refuses_missing_archive_and_missing_index() {
        let d = tempfile::tempdir().unwrap();
        let p = profile("w", d.path());
        let e = open_existing(&p, true).err().unwrap().to_string();
        assert!(e.contains("no archive found"), "{e}");
        assert!(
            !p.archive_dir.exists(),
            "the guard must not create the archive"
        );
        drop(archived(d.path()));
        std::fs::remove_file(p.archive_dir.join("index.db")).unwrap();
        let _ = std::fs::remove_file(p.archive_dir.join("index.db-wal"));
        let _ = std::fs::remove_file(p.archive_dir.join("index.db-shm"));
        let e = open_existing(&p, true).err().unwrap().to_string();
        assert!(e.contains("reindex w"), "{e}");
        assert!(open_existing(&p, false).is_ok());
    }

    #[test]
    fn status_verify_reindex_report_typed_results() {
        let d = tempfile::tempdir().unwrap();
        let (p, store) = archived(d.path());
        drop(store);
        let st = status(&p).unwrap();
        assert_eq!(
            st.folders,
            vec![
                FolderCount {
                    folder: "INBOX".into(),
                    messages: 3
                },
                FolderCount {
                    folder: "Sent".into(),
                    messages: 1
                }
            ]
        );
        assert_eq!((st.total, st.unique), (4, 4));
        let v = verify(&p).unwrap();
        assert_eq!(v.checked, 4);
        assert!(v.missing.is_empty() && v.corrupt.is_empty());
        assert_eq!(reindex(&p).unwrap(), 4);
    }

    #[test]
    fn archive_reports_failures_as_events_and_partial_class() {
        let d = tempfile::tempdir().unwrap();
        let mut p = profile("w", d.path());
        p.folders = Some(vec!["INBOX".into(), "Nope".into()]);
        let mut store = open_for_archive(&p).unwrap();
        let mut src = fake();
        src.fail_uids.insert(2);
        let (tx, rx) = channel();
        let r = archive_with(&p, &mut src, &mut store, None, &tx);
        drop(tx);
        let failures: Vec<Failure> = rx.iter().filter_map(|e| Failure::from_event(&e)).collect();
        assert_eq!(ExitClass::of(&r), ExitClass::Partial);
        assert_eq!(r.unwrap().added, 2);
        assert_eq!(failures.len(), 2);
        assert!(failures
            .iter()
            .any(|f| f.folder == "Nope" && f.uid.is_none()));
        assert!(failures
            .iter()
            .any(|f| f.folder == "INBOX" && f.uid == Some(2)));
    }

    #[test]
    fn cancelled_archive_stops_and_resumes_cleanly() {
        let d = tempfile::tempdir().unwrap();
        let p = profile("w", d.path());
        let mut store = open_for_archive(&p).unwrap();
        let flag = Arc::new(AtomicBool::new(true));
        let (tx, _rx) = channel();
        let r = archive_with(&p, &mut fake(), &mut store, Some(flag.clone()), &tx);
        assert!(matches!(&r, Err(e) if is_cancelled(e)), "{r:?}");
        assert_eq!(ExitClass::of(&r), ExitClass::Error);
        flag.store(false, Ordering::SeqCst);
        let s = archive_with(&p, &mut fake(), &mut store, Some(flag), &tx).unwrap();
        assert_eq!(s.added, 4);
    }

    #[test]
    fn export_to_eml_dir_is_incremental_and_cancellable() {
        let d = tempfile::tempdir().unwrap();
        let (mut p, mut store) = archived(d.path());
        let out = d.path().join("out");
        p.target = Some(TargetSpec::EmlDir(out.clone()));
        let secrets = MemorySecrets::new();
        let (tx, _rx) = channel();
        // cancelled before it starts: nothing is recorded as exported
        let flag = Arc::new(AtomicBool::new(true));
        let r = run_export(&p, &secrets, &mut store, None, Some(flag), &tx);
        assert!(r.is_err());
        let s = run_export(&p, &secrets, &mut store, Some(TargetKind::Eml), None, &tx).unwrap();
        assert_eq!((s.added, s.failed), (4, 0));
        let again = run_export(&p, &secrets, &mut store, None, None, &tx).unwrap();
        assert_eq!((again.added, again.duplicates), (0, 4));
        let e = run_export(&p, &secrets, &mut store, Some(TargetKind::Imap), None, &tx)
            .unwrap_err()
            .to_string();
        assert!(e.contains("does not match"), "{e}");
    }

    #[test]
    fn check_target_needs_a_target_and_the_imap_password() {
        let d = tempfile::tempdir().unwrap();
        let mut p = profile("w", d.path());
        let secrets = MemorySecrets::new();
        assert!(check_target(&p, &secrets, None)
            .unwrap_err()
            .to_string()
            .contains("no export target"));
        p.target = Some(TargetSpec::Imap(ep()));
        assert!(matches!(
            check_target(&p, &secrets, None),
            Err(Error::Auth(_))
        ));
        secrets
            .set(&secret_key("w", SecretRole::Target), "pw")
            .unwrap();
        check_target(&p, &secrets, Some(TargetKind::Imap)).unwrap();
    }

    #[test]
    fn create_update_remove_profile() {
        let d = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::new(&d.path().join("cfg"));
        let secrets = MemorySecrets::new();
        let mut p = profile("work", d.path());
        p.target = Some(TargetSpec::Imap(ep()));
        // an IMAP target without its password stores nothing
        assert!(create_profile(&profiles, &secrets, &p, "src", None).is_err());
        assert!(profiles.list().unwrap().is_empty());
        assert!(secrets
            .get(&secret_key("work", SecretRole::Source))
            .is_err());
        create_profile(&profiles, &secrets, &p, "src", Some("tgt")).unwrap();
        assert_eq!(
            secrets
                .get(&secret_key("work", SecretRole::Source))
                .unwrap(),
            "src"
        );
        assert_eq!(
            secrets
                .get(&secret_key("work", SecretRole::Target))
                .unwrap(),
            "tgt"
        );
        // duplicates, also ignoring case
        assert!(create_profile(&profiles, &secrets, &p, "x", Some("y")).is_err());
        let mut upper = p.clone();
        upper.name = "WORK".into();
        let e = create_profile(&profiles, &secrets, &upper, "x", Some("y"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("case-insensitive"), "{e}");
        // bad since or name
        let mut bad = p.clone();
        bad.name = "other".into();
        bad.since = Some("2024-13-01".into());
        assert!(create_profile(&profiles, &secrets, &bad, "x", Some("y")).is_err());
        bad.since = None;
        bad.name = "a b".into();
        assert!(create_profile(&profiles, &secrets, &bad, "x", Some("y")).is_err());
        // the toml never holds a password
        let toml = std::fs::read_to_string(d.path().join("cfg/profiles/work.toml")).unwrap();
        assert!(!toml.contains("tgt"), "{toml}");
        assert!(!toml.contains("password"));
        // update: since change without an archive does not reset anything
        p.since = Some("2024-01-01".into());
        assert!(!update_profile(&profiles, &p).unwrap());
        assert_eq!(
            profiles.load("work").unwrap().since.as_deref(),
            Some("2024-01-01")
        );
        remove_profile(&profiles, &secrets, "work").unwrap();
        assert!(profiles.list().unwrap().is_empty());
        assert!(secrets
            .get(&secret_key("work", SecretRole::Source))
            .is_err());
        assert!(secrets
            .get(&secret_key("work", SecretRole::Target))
            .is_err());
        assert!(remove_profile(&profiles, &secrets, "work").is_err());
    }

    #[test]
    fn update_with_new_since_rewinds_an_existing_archive() {
        let d = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::new(&d.path().join("cfg"));
        let (mut p, store) = archived(d.path());
        drop(store);
        profiles.save(&p).unwrap();
        p.since = Some("2020-01-01".into());
        assert!(update_profile(&profiles, &p).unwrap());
        // unchanged since: no reset
        assert!(!update_profile(&profiles, &p).unwrap());
    }

    #[test]
    fn remove_profile_keeps_the_archive() {
        let d = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::new(&d.path().join("cfg"));
        let secrets = MemorySecrets::new();
        let (p, store) = archived(d.path());
        drop(store);
        create_profile(&profiles, &secrets, &p, "pw", None).unwrap();
        remove_profile(&profiles, &secrets, "w").unwrap();
        assert!(archive_exists(&p.archive_dir));
    }

    #[test]
    fn absolutize_makes_paths_absolute() {
        let mut p = profile("w", Path::new("rel"));
        p.target = Some(TargetSpec::EmlDir("out".into()));
        absolutize(&mut p).unwrap();
        assert!(p.archive_dir.is_absolute());
        assert!(matches!(&p.target, Some(TargetSpec::EmlDir(d)) if d.is_absolute()));
    }

    #[test]
    fn test_connection_rejects_empty_fields_without_connecting() {
        let mut e = ep();
        e.host = " ".into();
        assert!(matches!(
            test_connection(&e, "pw".into()),
            Err(Error::Config(_))
        ));
        let mut e = ep();
        e.user = String::new();
        assert!(matches!(
            test_connection(&e, "pw".into()),
            Err(Error::Config(_))
        ));
    }
}
