//! End-to-end: Dovecot -> archive -> second Dovecot account, through the real `mailbackup` binary.
//!
//! `#[ignore]`d: it needs the Dovecot of `tests/dovecot` (see the README there) with a TLS
//! certificate the OS trusts, because mailbackup always validates certificates. Run with a fresh
//! container (`docker compose down && docker compose up -d`) because the test seeds `origem`:
//!
//! `cargo test -p mailbackup --features test-secrets --test e2e_dovecot -- --ignored --nocapture`
//!
//! Environment (defaults match `tests/dovecot/docker-compose.yml`):
//! `MAILBACKUP_E2E_HOST` (localhost), `MAILBACKUP_E2E_PORT` (993),
//! `MAILBACKUP_E2E_ORIGEM_USER` (origem), `MAILBACKUP_E2E_ORIGEM_PASSWORD` (origem-test-only),
//! `MAILBACKUP_E2E_DESTINO_USER` (destino), `MAILBACKUP_E2E_DESTINO_PASSWORD` (destino-test-only).
use assert_cmd::assert::Assert;
use assert_cmd::Command;
use mailbackup_core::{
    sha256_hex, ImapSource, ImapTarget, MailSource, MailTarget, PasswordAuth, TlsMode,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

type Snapshot = BTreeMap<String, Vec<(String, Vec<String>)>>;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

struct Server {
    host: String,
    port: u16,
}

fn server() -> Server {
    Server {
        host: env_or("MAILBACKUP_E2E_HOST", "localhost"),
        port: env_or("MAILBACKUP_E2E_PORT", "993").parse().unwrap(),
    }
}

/// A seed message: target folder, IMAP flags, internal date (epoch seconds) and the exact bytes.
struct Seed {
    folder: String,
    flags: Vec<String>,
    date: i64,
    bytes: Vec<u8>,
}

fn crlf(text: &str) -> Vec<u8> {
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .into_bytes()
}

fn flags_sidecar(eml: &Path) -> Vec<String> {
    std::fs::read_to_string(eml.with_extension("flags"))
        .map(|s| s.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

fn eml_files(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "eml"))
        .collect();
    v.sort();
    v
}

/// Every directory under `dir` is a folder (nested directories are nested folders, joined by "/").
fn collect_folders(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    for p in subdirs {
        let rel = p.strip_prefix(root).unwrap();
        let name: Vec<_> = rel.iter().map(|c| c.to_string_lossy()).collect();
        for f in eml_files(&p) {
            out.push((name.join("/"), f));
        }
        collect_folders(root, &p, out);
    }
}

/// Seeds from `tests/dovecot/seed` plus a generated message with a ~5 MB attachment.
fn seeds() -> Vec<Seed> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/dovecot/seed");
    let mut found: Vec<(String, PathBuf)> = eml_files(&root)
        .into_iter()
        .map(|f| ("INBOX".to_string(), f))
        .collect();
    let folders = root.join("folders");
    collect_folders(&folders, &folders, &mut found);
    let mut seeds: Vec<Seed> = found
        .into_iter()
        .enumerate()
        .map(|(i, (folder, f))| Seed {
            folder,
            flags: flags_sidecar(&f),
            date: 1_136_214_245 + i as i64 * 86_400,
            bytes: crlf(&std::fs::read_to_string(&f).unwrap()),
        })
        .collect();

    // ~5 MB of deterministic, incompressible-looking attachment data, base64 in 76-column lines.
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let raw: Vec<u8> = (0..3_800_000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect();
    let mut body = String::from(
        "Message-ID: <seed-big@mailbackup.test>\r\nFrom: Fabio <fabio@example.com>\r\n\
         To: origem@localhost\r\nSubject: Anexo grande\r\nDate: Sat, 07 Jan 2006 18:00:00 +0000\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"bnd\"\r\n\r\n\
         --bnd\r\nContent-Type: text/plain\r\n\r\nSegue o anexo.\r\n--bnd\r\n\
         Content-Type: application/octet-stream\r\nContent-Transfer-Encoding: base64\r\n\
         Content-Disposition: attachment; filename=\"dados.bin\"\r\n\r\n",
    );
    body.push_str(&base64(&raw));
    body.push_str("--bnd--\r\n");
    assert!(body.len() > 5_000_000, "attachment should be ~5 MB");
    seeds.push(Seed {
        folder: "INBOX".into(),
        flags: vec!["\\Seen".into()],
        date: 1_136_214_245 + 90 * 86_400,
        bytes: body.into_bytes(),
    });
    seeds
}

fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for line in data.chunks(57) {
        for c in line.chunks(3) {
            let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
            let v = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            out.push(T[(v >> 18) as usize & 63] as char);
            out.push(T[(v >> 12) as usize & 63] as char);
            out.push(if c.len() > 1 {
                T[(v >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if c.len() > 2 {
                T[v as usize & 63] as char
            } else {
                '='
            });
        }
        out.push_str("\r\n");
    }
    out
}

fn connect_source(s: &Server, user: &str, pw: &str) -> ImapSource {
    ImapSource::connect(
        &s.host,
        s.port,
        TlsMode::Implicit,
        Box::new(PasswordAuth::new(user, pw)),
    )
    .unwrap_or_else(|e| panic!("cannot connect/login as {user}: {e} (see tests/dovecot/README.md)"))
}

fn seed_account(s: &Server, user: &str, pw: &str, seeds: &[Seed]) {
    let mut t = ImapTarget::connect(
        &s.host,
        s.port,
        TlsMode::Implicit,
        Box::new(PasswordAuth::new(user, pw)),
    )
    .unwrap_or_else(|e| {
        panic!("cannot connect/login as {user}: {e} (see tests/dovecot/README.md)")
    });
    for m in seeds {
        t.put(&m.folder, &m.bytes, &m.flags, Some(m.date)).unwrap();
    }
}

/// folder -> sorted [(sha256 of the bytes, sorted flags)], ignoring empty folders and `\Recent`.
fn snapshot(s: &Server, user: &str, pw: &str) -> Snapshot {
    let mut src = connect_source(s, user, pw);
    let mut out = BTreeMap::new();
    for folder in src.list_folders().unwrap() {
        src.select(&folder).unwrap();
        let uids = src.uids_after(0, None).unwrap();
        let mut msgs = vec![];
        for chunk in uids.chunks(5) {
            for m in src.fetch(chunk).unwrap() {
                let mut flags: Vec<String> =
                    m.flags.into_iter().filter(|f| f != "\\Recent").collect();
                flags.sort();
                msgs.push((sha256_hex(&m.bytes), flags));
            }
        }
        if !msgs.is_empty() {
            msgs.sort();
            out.insert(folder, msgs);
        }
    }
    out
}

fn cmd(cfg: &Path) -> Command {
    let mut c = Command::cargo_bin("mailbackup").unwrap();
    c.arg("--config-dir").arg(cfg);
    c.env_remove("MAILBACKUP_CONFIG_DIR");
    c
}

fn stdout(a: &Assert) -> String {
    String::from_utf8_lossy(&a.get_output().stdout).into_owned()
}

#[test]
#[ignore = "requires the Dovecot of tests/dovecot (see the README there)"]
fn e2e_archive_then_export_to_second_account() {
    let s = server();
    let (ou, op) = (
        env_or("MAILBACKUP_E2E_ORIGEM_USER", "origem"),
        env_or("MAILBACKUP_E2E_ORIGEM_PASSWORD", "origem-test-only"),
    );
    let (du, dp) = (
        env_or("MAILBACKUP_E2E_DESTINO_USER", "destino"),
        env_or("MAILBACKUP_E2E_DESTINO_PASSWORD", "destino-test-only"),
    );
    let port = s.port.to_string();

    let seeds = seeds();
    assert!(seeds.len() >= 6);
    seed_account(&s, &ou, &op, &seeds);
    let before = snapshot(&s, &ou, &op);
    let seeded: usize = before.values().map(Vec::len).sum();
    assert_eq!(seeded, seeds.len(), "origem must start empty: {before:#?}");
    assert!(before.contains_key("\u{c1}lbum de Fotos"), "{before:#?}");
    assert!(before.contains_key("Trabalho/Projetos 2024"), "{before:#?}");
    assert!(
        snapshot(&s, &du, &dp).is_empty(),
        "destino must start empty (fresh container)"
    );

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("cfg");
    let archive_dir = dir.path().join("archive");
    cmd(&cfg)
        .args([
            "profile", "add", "origem", "--host", &s.host, "--tls", "implicit",
        ])
        .args(["--port", &port, "--user", &ou])
        .arg("--archive-dir")
        .arg(&archive_dir)
        .args([
            "--target-imap-host",
            &s.host,
            "--target-imap-tls",
            "implicit",
        ])
        .args(["--target-imap-port", &port, "--target-imap-user", &du])
        .arg("--password-stdin")
        .write_stdin(format!("{op}\n{dp}\n"))
        .assert()
        .success();

    let added = format!("added: {}", seeds.len());
    let a = cmd(&cfg).args(["archive", "origem"]).assert().success();
    assert!(stdout(&a).contains(&added), "{}", stdout(&a));
    cmd(&cfg).args(["verify", "origem"]).assert().success();

    let x = cmd(&cfg)
        .args(["export", "origem", "--to", "imap"])
        .assert()
        .success();
    assert!(stdout(&x).contains(&added), "{}", stdout(&x));

    // Same folders, same message counts, same flags and the same bytes (sha256) on both sides.
    let after = snapshot(&s, &du, &dp);
    assert_eq!(before, after);
    assert_eq!(after.values().map(Vec::len).sum::<usize>(), seeds.len());
    let seed_hashes: Vec<String> = seeds.iter().map(|m| sha256_hex(&m.bytes)).collect();
    for (h, _) in after.values().flatten() {
        assert!(seed_hashes.contains(h), "unexpected message {h}");
    }

    // A second export has nothing new to do and leaves the destination untouched.
    let again = cmd(&cfg)
        .args(["export", "origem", "--to", "imap"])
        .assert()
        .success();
    assert!(stdout(&again).contains("added: 0"), "{}", stdout(&again));
    assert_eq!(snapshot(&s, &du, &dp), after);
}
