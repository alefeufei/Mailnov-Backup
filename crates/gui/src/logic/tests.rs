use super::*;
use mailbackup_core::testing::FakeSource;
use mailbackup_core::{FolderState, MemorySecrets, RawMessage};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::Condvar;

// ---- fakes ----

type MakeSource = Box<dyn Fn() -> FakeSource + Send + Sync>;

struct FakeBackend {
    make: MakeSource,
    /// When set, `list_folders` waits until it is released (to observe a running operation).
    gate: Option<Arc<(Mutex<bool>, Condvar)>>,
    conn: Result<Vec<String>, fn() -> Error>,
}

impl Backend for FakeBackend {
    fn source(&self, p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailSource>, Error> {
        service::password(secrets, &p.name, SecretRole::Source)?;
        Ok(Box::new(GatedSource {
            inner: (self.make)(),
            gate: self.gate.clone(),
        }))
    }
    fn target(&self, p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailTarget>, Error> {
        service::target_for(p, secrets)
    }
    fn test_connection(&self, _ep: &Endpoint, _pw: String) -> Result<Vec<String>, Error> {
        match &self.conn {
            Ok(v) => Ok(v.clone()),
            Err(f) => Err(f()),
        }
    }
}

struct GatedSource {
    inner: FakeSource,
    gate: Option<Arc<(Mutex<bool>, Condvar)>>,
}

impl MailSource for GatedSource {
    fn list_folders(&mut self) -> Result<Vec<String>, Error> {
        if let Some(g) = &self.gate {
            let (lock, cv) = &**g;
            let mut open = lock.lock().unwrap();
            while !*open {
                open = cv.wait(open).unwrap();
            }
        }
        self.inner.list_folders()
    }
    fn select(&mut self, folder: &str) -> Result<FolderState, Error> {
        self.inner.select(folder)
    }
    fn uids_after(&mut self, last_uid: u32, since: Option<i64>) -> Result<Vec<u32>, Error> {
        self.inner.uids_after(last_uid, since)
    }
    fn message_ids(&mut self, uids: &[u32]) -> Result<Vec<(u32, Option<String>)>, Error> {
        self.inner.message_ids(uids)
    }
    fn fetch(&mut self, uids: &[u32]) -> Result<Vec<RawMessage>, Error> {
        self.inner.fetch(uids)
    }
    fn flags(&mut self, uids: &[u32]) -> Result<Vec<(u32, Vec<String>)>, Error> {
        self.inner.flags(uids)
    }
}

fn open_gate(g: &Arc<(Mutex<bool>, Condvar)>) {
    let (lock, cv) = &**g;
    *lock.lock().unwrap() = true;
    cv.notify_all();
}

struct RecSink {
    progress: Mutex<Vec<Progress>>,
    done: Mutex<SyncSender<Finished>>,
}

impl Sink for RecSink {
    fn progress(&self, p: &Progress) {
        self.progress.lock().unwrap().push(p.clone());
    }
    fn finished(&self, f: &Finished) {
        let _ = self.done.lock().unwrap().send(f.clone());
    }
}

fn sink() -> (Arc<RecSink>, Receiver<Finished>) {
    let (tx, rx) = std::sync::mpsc::sync_channel(4);
    (
        Arc::new(RecSink {
            progress: Mutex::new(vec![]),
            done: Mutex::new(tx),
        }),
        rx,
    )
}

fn msg(n: u32) -> Vec<u8> {
    format!("Message-ID: <m{n}@x>\r\nSubject: s{n}\r\n\r\nbody {n}\r\n").into_bytes()
}

fn mailbox() -> FakeSource {
    let mut s = FakeSource::new();
    s.add_folder("INBOX", 1);
    s.add_folder("Caixa de Saída", 1);
    for n in 1..=5 {
        s.add_message("INBOX", n, msg(n), &["\\Seen"]);
    }
    s.add_message("Caixa de Saída", 1, msg(100), &[]);
    s
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state: Arc<AppState>,
}

fn env_with(make: MakeSource, gate: Option<Arc<(Mutex<bool>, Condvar)>>) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let state = Arc::new(AppState::new(
        root.join("cfg"),
        Box::new(MemorySecrets::new()),
        Box::new(FakeBackend {
            make,
            gate,
            conn: Ok(vec!["INBOX".into(), "Caixa de Saída".into()]),
        }),
    ));
    Env {
        _dir: dir,
        root,
        state,
    }
}

fn env() -> Env {
    env_with(Box::new(mailbox), None)
}

fn ep_input() -> EndpointInput {
    EndpointInput {
        host: "imap.example.com".into(),
        port: 993,
        tls: "implicit".into(),
        user: "eu@example.com".into(),
    }
}

fn new_profile(e: &Env, name: &str) -> ProfileInput {
    ProfileInput {
        name: name.into(),
        source: ep_input(),
        password: Some("s3cret-pw".into()),
        folders: None,
        since: None,
        archive_dir: e.root.join("backup").join(name).display().to_string(),
        is_new: true,
    }
}

fn run(e: &Env, name: &str, op: Op) -> Finished {
    let (s, rx) = sink();
    start(e.state.clone(), name, op, s).unwrap();
    rx.recv_timeout(Duration::from_secs(20)).expect("finished")
}

// ---- tests ----

#[test]
fn detect_server_suggests_endpoint_name_dir_and_app_password_hint() {
    let e = env();
    let d = detect_server(&e.state, " Joao.Silva@gmail.com ").unwrap();
    assert_eq!(d.endpoint.host, "imap.gmail.com");
    assert_eq!(d.endpoint.port, 993);
    assert_eq!(d.endpoint.tls, "implicit");
    assert_eq!(d.endpoint.user, "Joao.Silva@gmail.com");
    assert_eq!(d.suggested_name, "Joao.Silva-gmail");
    assert!(d.suggested_dir.ends_with("Joao.Silva-gmail"));
    assert!(d.hint.unwrap().contains("senha de app"));
    assert!(detect_server(&e.state, "nope").is_err());
    let d = detect_server(&e.state, "a@empresa.com.br").unwrap();
    assert!(d.hint.is_none());
}

#[test]
fn suggested_names_are_valid_and_unique_ignoring_case() {
    let taken = vec!["eu-gmail".to_string(), "EU-GMAIL-2".to_string()];
    assert_eq!(suggest_name("eu@gmail.com", &taken), "eu-gmail-3");
    assert_eq!(suggest_name("joão+x@ex.com", &[]), "jo-o-x-ex");
    assert_eq!(suggest_name("...@...", &[]), "conta");
    let long = format!("{}@x.com", "a".repeat(100));
    let n = suggest_name(&long, &[]);
    assert!(mailbackup_core::profile::validate_name(&n).is_ok(), "{n}");
}

#[test]
fn test_connection_returns_folders_or_plain_language_error() {
    let e = env();
    let f = test_connection(&e.state, &ep_input(), "pw".into()).unwrap();
    assert_eq!(f, vec!["INBOX", "Caixa de Saída"]);
    assert!(test_connection(&e.state, &ep_input(), String::new()).is_err());
    let mut bad = ep_input();
    bad.host = "  ".into();
    assert!(test_connection(&e.state, &bad, "pw".into())
        .unwrap_err()
        .contains("servidor"));
    // a rejected login on Gmail adds the app-password hint, never the password
    let state = AppState::new(
        e.root.join("cfg2"),
        Box::new(MemorySecrets::new()),
        Box::new(FakeBackend {
            make: Box::new(mailbox),
            gate: None,
            conn: Err(|| Error::Auth("login rejected by server: NO [AUTHENTICATIONFAILED]".into())),
        }),
    );
    let mut gmail = ep_input();
    gmail.host = "imap.gmail.com".into();
    let msg = test_connection(&state, &gmail, "hunter2-secret".into()).unwrap_err();
    assert!(msg.contains("recusou o login"), "{msg}");
    assert!(msg.contains("senha de app"), "{msg}");
    assert!(!msg.contains("hunter2-secret"), "{msg}");
}

#[test]
fn saved_connection_uses_the_stored_password() {
    let e = env();
    assert!(
        test_saved_connection(&e.state, "work", "source", &ep_input())
            .unwrap_err()
            .contains("Credenciais")
    );
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let f = test_saved_connection(&e.state, "work", "source", &ep_input()).unwrap();
    assert_eq!(f.len(), 2);
    assert!(test_saved_connection(&e.state, "work", "target", &ep_input()).is_err());
}

#[test]
fn save_profile_stores_password_in_secret_store_only() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    assert_eq!(
        e.state
            .secrets
            .get(&secret_key("work", SecretRole::Source))
            .unwrap(),
        "s3cret-pw"
    );
    let toml = std::fs::read_to_string(e.root.join("cfg/profiles/work.toml")).unwrap();
    assert!(!toml.contains("s3cret-pw"), "{toml}");
    let list = list_profiles(&e.state).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].source.user, "eu@example.com");
    assert!(!list[0].archive_exists);
    assert_eq!(list[0].target_kind, "none");
    let json = serde_json::to_string(&list).unwrap();
    assert!(!json.contains("s3cret-pw"));
}

#[test]
fn save_profile_validates_name_case_collision_path_since_and_password() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let err = save_profile(&e.state, new_profile(&e, "WORK")).unwrap_err();
    assert!(err.contains("\"work\""), "{err}");
    assert!(save_profile(&e.state, new_profile(&e, "work")).is_err());
    assert!(save_profile(&e.state, new_profile(&e, "a b")).is_err());
    let mut rel = new_profile(&e, "rel");
    rel.archive_dir = "relative\\dir".into();
    assert!(save_profile(&e.state, rel)
        .unwrap_err()
        .contains("caminho completo"));
    let mut since = new_profile(&e, "since");
    since.since = Some("2024-02-30".into());
    assert!(save_profile(&e.state, since).unwrap_err().contains("Data"));
    let mut nopw = new_profile(&e, "nopw");
    nopw.password = None;
    assert!(save_profile(&e.state, nopw).is_err());
    assert_eq!(e.state.profiles.list().unwrap(), vec!["work".to_string()]);
    assert!(e
        .state
        .secrets
        .get(&secret_key("nopw", SecretRole::Source))
        .is_err());
}

#[test]
fn editing_keeps_password_unless_given_and_folders_since_are_saved() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let mut edit = new_profile(&e, "work");
    edit.is_new = false;
    edit.password = None;
    edit.folders = Some(vec!["INBOX".into(), " ".into()]);
    edit.since = Some("2024-01-01".into());
    save_profile(&e.state, edit.clone()).unwrap();
    let p = e.state.profiles.load("work").unwrap();
    assert_eq!(p.folders, Some(vec!["INBOX".to_string()]));
    assert_eq!(p.since.as_deref(), Some("2024-01-01"));
    let key = secret_key("work", SecretRole::Source);
    assert_eq!(e.state.secrets.get(&key).unwrap(), "s3cret-pw");
    edit.password = Some("new-pw".into());
    save_profile(&e.state, edit).unwrap();
    assert_eq!(e.state.secrets.get(&key).unwrap(), "new-pw");
}

#[test]
fn archive_runs_in_background_reports_progress_and_records_history() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let (s, rx) = sink();
    start(e.state.clone(), "work", Op::Archive, s.clone()).unwrap();
    let f = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(f.class, "ok", "{f:?}");
    assert_eq!((f.added, f.failed), (6, 0));
    assert_eq!(f.phases.len(), 1);
    let progress = s.progress.lock().unwrap();
    let last = progress.last().unwrap();
    assert_eq!(last.processed, 6);
    assert_eq!(last.phase, "archive");
    assert!(progress.iter().any(|p| p.phase == "connect"));
    drop(progress);
    let list = list_profiles(&e.state).unwrap();
    assert!(list[0].archive_exists);
    assert_eq!(list[0].messages, Some(6));
    assert_eq!(list[0].folder_count, Some(2));
    assert!(list[0].last_archive_at.is_some());
    assert_eq!(list[0].last_run.as_ref().unwrap().class, "ok");
    assert!(list[0].running.is_none());
    let st = get_status(&e.state, "work").unwrap();
    assert_eq!(st.unique, 6);
    assert!(st.folders.iter().any(|f| f.folder == "Caixa de Saída"));
    // again: nothing new
    let again = run(&e, "work", Op::Archive);
    assert_eq!(
        (again.class.as_str(), again.added, again.duplicates),
        ("ok", 0, 0)
    );
}

#[test]
fn failed_messages_make_a_partial_result_with_reasons() {
    let e = env_with(
        Box::new(|| {
            let mut s = mailbox();
            s.fail_uids.insert(3);
            s
        }),
        None,
    );
    let mut p = new_profile(&e, "work");
    p.folders = Some(vec!["INBOX".into(), "Nao Existe".into()]);
    save_profile(&e.state, p).unwrap();
    let f = run(&e, "work", Op::Archive);
    assert_eq!(f.class, "partial");
    assert_eq!(f.failed, 2);
    assert_eq!(f.failures_total, 2);
    assert!(f.failures.iter().any(|x| x.uid == Some(3)));
    assert!(f
        .failures
        .iter()
        .any(|x| x.folder == "Nao Existe" && x.reason.contains("not found")));
    let list = list_profiles(&e.state).unwrap();
    assert_eq!(list[0].last_run.as_ref().unwrap().class, "partial");
}

#[test]
fn one_operation_per_profile_and_cancel_stops_it() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let e = env_with(Box::new(mailbox), Some(gate.clone()));
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let (s, rx) = sink();
    start(e.state.clone(), "work", Op::Archive, s.clone()).unwrap();
    assert_eq!(e.state.running_op("work"), Some(Op::Archive));
    let second = start(e.state.clone(), "work", Op::Archive, s.clone()).unwrap_err();
    assert!(second.contains("em andamento"), "{second}");
    assert!(delete_profile(&e.state, "work").is_err());
    assert!(run_verify(&e.state, "work").is_err());
    let mut edit = new_profile(&e, "work");
    edit.is_new = false;
    assert!(save_profile(&e.state, edit).is_err());
    let busy = list_profiles(&e.state).unwrap();
    assert_eq!(busy[0].running, Some(Op::Archive));
    assert!(busy[0].messages.is_none());
    cancel(&e.state, "work").unwrap();
    open_gate(&gate);
    let f = rx.recv_timeout(Duration::from_secs(20)).unwrap();
    assert_eq!(f.class, "cancelled", "{f:?}");
    assert!(
        f.failures.is_empty(),
        "a cancellation is not a failure: {f:?}"
    );
    assert!(f.error.unwrap().contains("Cancelado"));
    assert!(e.state.running_op("work").is_none());
    assert!(cancel(&e.state, "work").is_err());
    // the next run resumes and completes
    let f = run(&e, "work", Op::Archive);
    assert_eq!((f.class.as_str(), f.added), ("ok", 6));
}

#[test]
fn export_needs_target_and_archive_then_exports_to_eml_dir() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let (s, _rx) = sink();
    let err = start(e.state.clone(), "work", Op::Export, s.clone()).unwrap_err();
    assert!(err.contains("destino"), "{err}");
    let out = e.root.join("eml-out");
    let t = |dir: &Path| TargetInput {
        kind: "eml".into(),
        dir: Some(dir.display().to_string()),
        endpoint: None,
        password: None,
    };
    // never inside the archive
    let inside = e.root.join("backup").join("work").join("x");
    assert!(set_target(&e.state, "work", t(&inside)).is_err());
    set_target(&e.state, "work", t(&out)).unwrap();
    let err = start(e.state.clone(), "work", Op::Export, s).unwrap_err();
    assert!(err.contains("Arquivar agora"), "{err}");
    let f = run(&e, "work", Op::Migrate);
    assert_eq!(f.class, "ok", "{f:?}");
    assert_eq!(f.phases.len(), 2);
    assert_eq!(f.phases[0].added, 6);
    assert_eq!(f.phases[1].added, 6);
    let emls = walk(&out);
    assert_eq!(emls, 6);
    let again = run(&e, "work", Op::Export);
    assert_eq!(
        (again.class.as_str(), again.added, again.duplicates),
        ("ok", 0, 6)
    );
    let list = list_profiles(&e.state).unwrap();
    assert_eq!(list[0].target_kind, "eml");
    assert_eq!(list[0].last_run.as_ref().unwrap().op, Op::Export);
}

fn walk(dir: &Path) -> usize {
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            n += walk(&p);
        } else if p.extension().is_some_and(|x| x == "eml") {
            n += 1;
        }
    }
    n
}

#[test]
fn imap_target_password_goes_to_secret_store() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    let t = |pw: Option<&str>| TargetInput {
        kind: "imap".into(),
        dir: None,
        endpoint: Some(EndpointInput {
            host: "imap.destino.com".into(),
            port: 143,
            tls: "starttls".into(),
            user: "eu@destino.com".into(),
        }),
        password: pw.map(Into::into),
    };
    assert!(set_target(&e.state, "work", t(None))
        .unwrap_err()
        .contains("senha"));
    set_target(&e.state, "work", t(Some("tgt-pw"))).unwrap();
    let key = secret_key("work", SecretRole::Target);
    assert_eq!(e.state.secrets.get(&key).unwrap(), "tgt-pw");
    // keeping the stored password
    set_target(&e.state, "work", t(None)).unwrap();
    let toml = std::fs::read_to_string(e.root.join("cfg/profiles/work.toml")).unwrap();
    assert!(
        !toml.contains("tgt-pw") && toml.contains("starttls"),
        "{toml}"
    );
    let list = list_profiles(&e.state).unwrap();
    assert_eq!(list[0].target_kind, "imap");
    assert_eq!(list[0].target_endpoint.as_ref().unwrap().port, 143);
    // switching to "none" forgets the target password
    let none = TargetInput {
        kind: "none".into(),
        dir: None,
        endpoint: None,
        password: None,
    };
    set_target(&e.state, "work", none).unwrap();
    assert!(e.state.secrets.get(&key).is_err());
}

#[test]
fn delete_profile_removes_secrets_and_history_but_never_the_archive() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    run(&e, "work", Op::Archive);
    let p = e.state.profiles.load("work").unwrap();
    delete_profile(&e.state, "work").unwrap();
    assert!(e.state.profiles.list().unwrap().is_empty());
    assert!(e
        .state
        .secrets
        .get(&secret_key("work", SecretRole::Source))
        .is_err());
    assert!(service::archive_exists(&p.archive_dir));
    assert!(!run_file(&e.state, "work").exists());
    assert!(delete_profile(&e.state, "work").is_err());
}

#[test]
fn verify_and_reindex_need_an_archive() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    assert!(run_verify(&e.state, "work")
        .unwrap_err()
        .contains("Arquivar agora"));
    assert!(run_reindex(&e.state, "work").is_err());
    assert_eq!(get_status(&e.state, "work").unwrap().total, 0);
    run(&e, "work", Op::Archive);
    let v = run_verify(&e.state, "work").unwrap();
    assert_eq!(v.checked, 6);
    assert!(v.missing.is_empty() && v.corrupt.is_empty());
    assert_eq!(run_reindex(&e.state, "work").unwrap(), 6);
}

#[test]
fn missing_password_is_reported_before_starting() {
    let e = env();
    save_profile(&e.state, new_profile(&e, "work")).unwrap();
    e.state
        .secrets
        .delete(&secret_key("work", SecretRole::Source))
        .unwrap();
    let (s, _rx) = sink();
    let msg = start(e.state.clone(), "work", Op::Archive, s).unwrap_err();
    assert!(msg.contains("Gerenciador de Credenciais"), "{msg}");
    assert!(e.state.running_op("work").is_none());
}

#[test]
fn tracker_counts_and_ignores_cancellation_failures() {
    let mut t = Tracker::new("w", Op::Archive, "archive");
    t.on_event(&Event::FolderStarted {
        folder: "INBOX".into(),
        total: 3,
    });
    t.on_event(&Event::MessageArchived {
        folder: "INBOX".into(),
        uid: 1,
    });
    t.on_event(&Event::MessageFailed {
        folder: "INBOX".into(),
        uid: 2,
        reason: "bad".into(),
    });
    t.on_event(&Event::MessageFailed {
        folder: "INBOX".into(),
        uid: 3,
        reason: Error::Connection("cancelled by user".into()).to_string(),
    });
    let p = t.snapshot();
    assert_eq!((p.done, p.total, p.processed, p.failed), (2, 3, 1, 1));
    assert_eq!(p.new_failures.len(), 1);
    assert!(t.snapshot().new_failures.is_empty());
    t.on_event(&Event::FolderStarted {
        folder: "Sent".into(),
        total: 1,
    });
    assert_eq!(t.state.folders_done, 1);
    assert_eq!(t.state.done, 0);
}

#[test]
fn progress_is_throttled_and_the_final_state_is_always_sent() {
    let (s, _rx) = sink();
    let mut t = Tracker::new("w", Op::Archive, "archive");
    let flag = AtomicBool::new(false);
    with_events(&mut t, s.as_ref(), &flag, |tx| {
        tx.send(Event::FolderStarted {
            folder: "INBOX".into(),
            total: 5000,
        })
        .unwrap();
        for uid in 0..5000 {
            tx.send(Event::MessageArchived {
                folder: "INBOX".into(),
                uid,
            })
            .unwrap();
        }
    });
    let progress = s.progress.lock().unwrap();
    assert!(progress.len() <= 5, "{} events", progress.len());
    let last = progress.last().unwrap();
    assert_eq!((last.done, last.total), (5000, 5000));
}

#[test]
fn humanize_explains_errors_in_portuguese_with_detail() {
    let m = humanize(&Error::Connection("connection refused".into()));
    assert!(m.contains("Não foi possível falar com o servidor"), "{m}");
    assert!(m.contains("connection refused"), "{m}");
    let m = humanize(&Error::Connection(
        "invalid peer certificate: NotValidForName".into(),
    ));
    assert!(m.contains("certificado"), "{m}");
    assert_eq!(
        humanize(&Error::Connection("cancelled by user".into())),
        "Cancelado pelo usuário."
    );
    assert!(humanize(&Error::Auth("no password stored for 'x'".into())).contains("Credenciais"));
}

#[test]
fn only_existing_absolute_directories_can_be_opened() {
    let d = tempfile::tempdir().unwrap();
    assert!(folder_to_open(&d.path().display().to_string()).is_ok());
    let file = d.path().join("x.exe");
    std::fs::write(&file, b"MZ").unwrap();
    assert!(folder_to_open(&file.display().to_string()).is_err());
    assert!(folder_to_open("relative").is_err());
    assert!(folder_to_open("").is_err());
    assert!(folder_to_open(&d.path().join("nope").display().to_string()).is_err());
}
