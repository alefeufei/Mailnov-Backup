//! End-to-end CLI tests. Run with `--features test-secrets` (plaintext file secret store); no network.
use assert_cmd::assert::Assert;
use assert_cmd::Command;
use mailbackup_core::store::{FsSqliteStore, Store};
use mailbackup_core::RawMessage;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

struct Env {
    _d: TempDir,
    cfg: PathBuf,
    archive: PathBuf,
}

fn env() -> Env {
    let d = tempfile::tempdir().unwrap();
    let cfg = d.path().join("cfg");
    let archive = d.path().join("archive");
    Env {
        _d: d,
        cfg,
        archive,
    }
}

fn cmd(e: &Env) -> Command {
    let mut c = Command::cargo_bin("mailbackup").unwrap();
    c.arg("--config-dir").arg(&e.cfg);
    c.env_remove("MAILBACKUP_CONFIG_DIR");
    c
}

fn add(e: &Env, name: &str, extra: &[&str]) -> Assert {
    cmd(e)
        .args([
            "profile",
            "add",
            name,
            "--host",
            "imap.example.com",
            "--user",
            "u@example.com",
        ])
        .arg("--archive-dir")
        .arg(&e.archive)
        .args(extra)
        .write_stdin("s3cret-pw\n")
        .assert()
}

fn secret_path(e: &Env, key: &str) -> PathBuf {
    let hex: String = key.bytes().map(|b| format!("{b:02x}")).collect();
    e.cfg.join("test-secrets").join(hex)
}

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

/// Archive populated through the core crate: 2 in INBOX, 1 in Sent.
fn populate(e: &Env, account: &str) {
    let mut s = FsSqliteStore::open(&e.archive).unwrap();
    s.commit_batch(account, "INBOX", 1, &[raw(1, 1), raw(2, 2)], 2)
        .unwrap();
    s.commit_batch(account, "Sent", 1, &[raw(1, 3)], 1).unwrap();
}

fn walk_eml(root: &Path) -> Vec<PathBuf> {
    fn go(d: &Path, out: &mut Vec<PathBuf>) {
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
    go(root, &mut v);
    v
}

fn out(a: &Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stdout).into_owned()
}

fn err(a: &Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stderr).into_owned()
}

#[test]
fn profile_add_list_rm_cycle() {
    let e = env();
    add(&e, "work", &["--folder", "INBOX", "--since", "2024-01-31"]).success();
    add(&e, "home", &[]).success();
    let o = out(&cmd(&e).args(["profile", "list"]).assert().success());
    assert!(o.contains("work") && o.contains("home"), "{o}");
    let o = out(&cmd(&e).args(["profile", "show", "work"]).assert().success());
    assert!(
        o.contains("imap.example.com") && o.contains("2024-01-31") && o.contains("INBOX"),
        "{o}"
    );
    // adding again fails
    add(&e, "work", &[]).failure().code(1);
    // edit
    cmd(&e)
        .args([
            "profile",
            "edit",
            "work",
            "--since",
            "2025-02-03",
            "--target-eml",
        ])
        .arg(e.cfg.join("out"))
        .assert()
        .success();
    let o = out(&cmd(&e).args(["profile", "show", "work"]).assert().success());
    assert!(o.contains("2025-02-03") && o.contains("eml"), "{o}");
    cmd(&e)
        .args(["profile", "edit", "work", "--since", "nope"])
        .assert()
        .failure()
        .code(1);
    cmd(&e).args(["profile", "rm", "work"]).assert().success();
    let o = out(&cmd(&e).args(["profile", "list"]).assert().success());
    assert!(!o.contains("work") && o.contains("home"), "{o}");
    cmd(&e)
        .args(["profile", "rm", "work"])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn profile_rm_keeps_archive_files() {
    let e = env();
    add(&e, "home", &[]).success();
    populate(&e, "home");
    let before = walk_eml(&e.archive);
    assert_eq!(before.len(), 3);
    cmd(&e).args(["profile", "rm", "home"]).assert().success();
    assert!(e.archive.join("archive.toml").is_file());
    assert!(e.archive.join("index.db").is_file());
    for f in &before {
        assert!(f.is_file(), "{f:?}");
    }
}

#[test]
fn profile_add_rejects_password_in_args() {
    let e = env();
    let a = cmd(&e)
        .args([
            "profile",
            "add",
            "w",
            "--host",
            "h",
            "--user",
            "u",
            "--archive-dir",
            "x",
            "--password",
            "hunter2",
        ])
        .assert()
        .failure()
        .code(1);
    assert!(
        err(&a).contains("unexpected argument '--password'"),
        "{}",
        err(&a)
    );
    assert!(!e.cfg.join("profiles").join("w.toml").exists());

    // set-password on an EXISTING profile: only the flag can make it fail
    add(&e, "ex", &[]).success();
    let key = "mailbackup:ex:source";
    let a = cmd(&e)
        .args(["profile", "set-password", "ex", "--password", "hunter2"])
        .write_stdin(
            "changed
",
        )
        .assert()
        .failure()
        .code(1);
    assert!(
        err(&a).contains("unexpected argument '--password'"),
        "{}",
        err(&a)
    );
    assert_eq!(
        std::fs::read_to_string(secret_path(&e, key)).unwrap(),
        "s3cret-pw"
    );
}

#[test]
fn profile_add_reads_password_from_stdin_and_never_prints_it() {
    let e = env();
    let a = add(&e, "w", &["--password-stdin"]).success();
    let text = format!("{}{}", out(&a), err(&a));
    assert!(!text.contains("s3cret-pw"), "{text}");
    let toml = std::fs::read_to_string(e.cfg.join("profiles").join("w.toml")).unwrap();
    assert!(!toml.contains("s3cret-pw"), "{toml}");
    let key = "mailbackup:w:source";
    assert_eq!(
        std::fs::read_to_string(secret_path(&e, key)).unwrap(),
        "s3cret-pw"
    );
    let s = cmd(&e).args(["profile", "show", "w"]).assert().success();
    assert!(!out(&s).contains("s3cret-pw"));
    cmd(&e)
        .args(["profile", "set-password", "w"])
        .write_stdin("other-pw\r\n")
        .assert()
        .success();
    assert_eq!(
        std::fs::read_to_string(secret_path(&e, key)).unwrap(),
        "other-pw"
    );
    cmd(&e).args(["profile", "rm", "w"]).assert().success();
    assert!(!secret_path(&e, key).exists());
}

#[test]
fn profile_add_rejects_invalid_name_and_bad_since() {
    let e = env();
    add(&e, "../evil", &[]).failure().code(1);
    add(&e, "con", &[]).failure().code(1);
    add(&e, "ok", &["--since", "2024-13-40"]).failure().code(1);
    let o = out(&cmd(&e).args(["profile", "list"]).assert().success());
    assert!(o.trim().is_empty() || o.contains("no profiles"), "{o}");
    assert!(
        !e.cfg.join("test-secrets").exists()
            || std::fs::read_dir(e.cfg.join("test-secrets"))
                .unwrap()
                .next()
                .is_none()
    );
}

#[test]
fn profile_add_with_email_guesses_server() {
    let e = env();
    cmd(&e)
        .args([
            "profile",
            "add",
            "g",
            "--email",
            "me@gmail.com",
            "--archive-dir",
        ])
        .arg(&e.archive)
        .write_stdin("pw\n")
        .assert()
        .success();
    let o = out(&cmd(&e).args(["profile", "show", "g"]).assert().success());
    assert!(o.contains("imap.gmail.com") && o.contains("993"), "{o}");
}

#[test]
fn verify_exit_code_is_1_when_eml_missing_and_0_when_intact() {
    let e = env();
    add(&e, "w", &[]).success();
    populate(&e, "w");
    let v = cmd(&e).args(["verify", "w"]).assert().success();
    assert!(out(&v).contains("checked: 3"), "{}", out(&v));
    let files = walk_eml(&e.archive);
    assert_eq!(files.len(), 3);
    std::fs::remove_file(&files[0]).unwrap();
    let v = cmd(&e).args(["verify", "w"]).assert().failure().code(1);
    assert!(out(&v).contains("missing: 1"), "{}", out(&v));
}

#[test]
fn reindex_recovers_after_deleting_index_db() {
    let e = env();
    add(&e, "w", &[]).success();
    populate(&e, "w");
    for f in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = std::fs::remove_file(e.archive.join(f));
    }
    let r = cmd(&e).args(["reindex", "w"]).assert().success();
    assert!(out(&r).contains("indexed 3 messages"), "{}", out(&r));
    let s = cmd(&e).args(["status", "w"]).assert().success();
    assert!(out(&s).contains("INBOX: 2"), "{}", out(&s));
}

#[test]
fn status_prints_counts_per_folder() {
    let e = env();
    add(&e, "w", &[]).success();
    populate(&e, "w");
    let o = out(&cmd(&e).args(["status", "w"]).assert().success());
    assert!(
        o.contains("INBOX: 2") && o.contains("Sent: 1") && o.contains("total: 3"),
        "{o}"
    );
}

#[test]
fn export_to_eml_runs_offline_from_existing_archive() {
    let e = env();
    let dest = e.cfg.join("eml-out");
    add(&e, "w", &[]).success();
    cmd(&e)
        .args(["profile", "edit", "w", "--target-eml"])
        .arg(&dest)
        .assert()
        .success();
    populate(&e, "w");
    let r = cmd(&e)
        .args(["export", "w", "--to", "eml"])
        .assert()
        .success();
    assert!(out(&r).contains("added: 3"), "{}", out(&r));
    assert_eq!(walk_eml(&dest).len(), 3);
    let o = out(&cmd(&e)
        .args(["export", "w", "--to", "eml"])
        .assert()
        .success());
    assert!(o.contains("added: 0") && o.contains("duplicates: 3"), "{o}");
    assert_eq!(walk_eml(&dest).len(), 3);
    cmd(&e)
        .args(["export", "w", "--to", "imap"])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn migrate_without_target_is_an_error() {
    let e = env();
    add(&e, "w", &[]).success();
    let r = cmd(&e).args(["migrate", "w"]).assert().failure().code(1);
    assert!(err(&r).contains("target"), "{}", err(&r));
}

#[test]
fn archive_without_stored_password_is_an_error_without_network() {
    let e = env();
    add(&e, "w", &[]).success();
    std::fs::remove_file(secret_path(&e, "mailbackup:w:source")).unwrap();
    let r = cmd(&e).args(["archive", "w"]).assert().failure().code(1);
    assert!(!err(&r).contains("s3cret-pw"));
}

#[test]
fn help_and_missing_profile() {
    let e = env();
    cmd(&e).arg("--help").assert().success();
    cmd(&e).args(["status", "nope"]).assert().failure().code(1);
}

fn assert_no_archive(e: &Env, a: &Assert) {
    let msg = err(a);
    let path = e.archive.display().to_string();
    assert!(msg.contains(&path), "stderr should name {path}: {msg}");
    assert!(!e.archive.exists(), "archive dir must not be created");
}

#[test]
fn read_commands_on_missing_archive_fail_without_creating_it() {
    let e = env();
    add(&e, "w", &[]).success();
    cmd(&e)
        .args(["profile", "edit", "w", "--target-eml"])
        .arg(e.cfg.join("eml-out"))
        .assert()
        .success();
    for c in ["verify", "status", "reindex"] {
        let a = cmd(&e).args([c, "w"]).assert().failure().code(1);
        assert_no_archive(&e, &a);
    }
    let a = cmd(&e)
        .args(["export", "w", "--to", "eml"])
        .assert()
        .failure()
        .code(1);
    assert_no_archive(&e, &a);
}

#[test]
fn target_errors_leave_no_archive_behind() {
    let e = env();
    add(&e, "w", &[]).success();
    // no target at all
    cmd(&e)
        .args(["export", "w", "--to", "eml"])
        .assert()
        .failure()
        .code(1);
    cmd(&e).args(["migrate", "w"]).assert().failure().code(1);
    assert!(!e.archive.exists());
    // kind mismatch
    cmd(&e)
        .args(["profile", "edit", "w", "--target-eml"])
        .arg(e.cfg.join("eml-out"))
        .assert()
        .success();
    cmd(&e)
        .args(["export", "w", "--to", "imap"])
        .assert()
        .failure()
        .code(1);
    assert!(!e.archive.exists());
}

#[test]
fn migrate_fails_fast_when_target_password_missing() {
    let e = env();
    add(&e, "w", &[]).success();
    cmd(&e)
        .args([
            "profile",
            "edit",
            "w",
            "--target-imap-host",
            "t.example.com",
            "--target-imap-user",
            "tu",
        ])
        .assert()
        .success();
    let a = cmd(&e).args(["migrate", "w"]).assert().failure().code(1);
    assert!(err(&a).contains("--target"), "{}", err(&a));
    assert!(!e.archive.exists());
}

#[test]
fn reindex_works_when_only_index_db_deleted() {
    let e = env();
    add(&e, "w", &[]).success();
    populate(&e, "w");
    for f in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = std::fs::remove_file(e.archive.join(f));
    }
    assert!(e.archive.join("archive.toml").is_file());
    cmd(&e).args(["reindex", "w"]).assert().success();
}

fn delete_only_index_db(e: &Env) {
    for f in ["index.db", "index.db-wal", "index.db-shm"] {
        let _ = std::fs::remove_file(e.archive.join(f));
    }
    assert!(e.archive.join("archive.toml").is_file());
}

#[test]
fn verify_and_status_refuse_when_only_index_db_deleted() {
    for c in ["verify", "status"] {
        let e = env();
        add(&e, "w", &[]).success();
        populate(&e, "w");
        delete_only_index_db(&e);
        let a = cmd(&e).args([c, "w"]).assert().failure().code(1);
        assert!(err(&a).contains("reindex w"), "{c}: {}", err(&a));
        assert!(
            !out(&a).contains("total:") && !out(&a).contains("checked:"),
            "{c} must not report an empty archive: {}",
            out(&a)
        );
        assert!(
            !e.archive.join("index.db").exists(),
            "{c} must not recreate index.db"
        );
        // reindex still recovers, after which the command works.
        cmd(&e).args(["reindex", "w"]).assert().success();
        cmd(&e).args([c, "w"]).assert().success();
    }
}

// ---- final fix wave ----

fn folder_last_uids(e: &Env, account: &str) -> Vec<(String, u32, u32)> {
    let s = FsSqliteStore::open(&e.archive).unwrap();
    ["INBOX", "Sent"]
        .iter()
        .map(|f| {
            let st = s.folder_state(account, f).unwrap().unwrap();
            (f.to_string(), st.uidvalidity, st.last_uid)
        })
        .collect()
}

#[test]
fn changing_since_resets_last_uid_so_older_mail_is_backfilled() {
    let e = env();
    add(&e, "w", &["--since", "2024-01-01"]).success();
    populate(&e, "w");
    let untouched = vec![("INBOX".to_string(), 1, 2), ("Sent".to_string(), 1, 1)];
    // same value or unrelated edits: nothing is reset
    cmd(&e)
        .args([
            "profile",
            "edit",
            "w",
            "--since",
            "2024-01-01",
            "--folder",
            "INBOX",
        ])
        .assert()
        .success();
    assert_eq!(folder_last_uids(&e, "w"), untouched);
    cmd(&e)
        .args(["profile", "edit", "w", "--clear-since"])
        .assert()
        .success();
    let reset = vec![("INBOX".to_string(), 1, 0), ("Sent".to_string(), 1, 0)];
    assert_eq!(folder_last_uids(&e, "w"), reset);
    assert_eq!(
        FsSqliteStore::open(&e.archive)
            .unwrap()
            .messages("w")
            .unwrap()
            .len(),
        3
    );
    // moving the date also resets
    populate(&e, "w");
    cmd(&e)
        .args(["profile", "edit", "w", "--since", "2023-05-06"])
        .assert()
        .success();
    assert_eq!(folder_last_uids(&e, "w"), reset);
}

#[test]
fn changing_since_without_an_archive_creates_nothing() {
    let e = env();
    add(&e, "w", &["--since", "2024-01-01"]).success();
    cmd(&e)
        .args(["profile", "edit", "w", "--clear-since"])
        .assert()
        .success();
    assert!(!e.archive.exists());
}

#[test]
fn relative_paths_are_stored_absolute() {
    let e = env();
    let cwd = e.cfg.parent().unwrap().join("work-dir");
    std::fs::create_dir_all(&cwd).unwrap();
    cmd(&e)
        .current_dir(&cwd)
        .args([
            "profile",
            "add",
            "w",
            "--host",
            "h",
            "--user",
            "u",
            "--archive-dir",
            "rel-archive",
            "--target-eml",
            "rel-out",
        ])
        .write_stdin("pw\n")
        .assert()
        .success();
    let o = out(&cmd(&e).args(["profile", "show", "w"]).assert().success());
    let want = |p: &str| {
        std::path::absolute(cwd.join(p))
            .unwrap()
            .display()
            .to_string()
    };
    assert!(
        o.contains(&format!("archive_dir: {}", want("rel-archive"))),
        "{o}"
    );
    assert!(
        o.contains(&format!("target: eml {}", want("rel-out"))),
        "{o}"
    );
    cmd(&e)
        .current_dir(&cwd)
        .args([
            "profile",
            "edit",
            "w",
            "--archive-dir",
            "other",
            "--target-eml",
            "o2",
        ])
        .assert()
        .success();
    let o = out(&cmd(&e).args(["profile", "show", "w"]).assert().success());
    assert!(
        o.contains(&format!("archive_dir: {}", want("other"))),
        "{o}"
    );
    assert!(o.contains(&format!("target: eml {}", want("o2"))), "{o}");
}

#[test]
fn profile_names_differing_only_by_case_are_rejected() {
    let e = env();
    add(&e, "work", &["--since", "2024-01-01"]).success();
    let a = add(&e, "Work", &[]).failure().code(1);
    assert!(err(&a).contains("work"), "{}", err(&a));
    let o = out(&cmd(&e).args(["profile", "show", "work"]).assert().success());
    assert!(o.contains("2024-01-01"), "{o}");
    let o = out(&cmd(&e).args(["profile", "list"]).assert().success());
    assert_eq!(o.trim(), "work");
}
