//! Thin command wrappers around `mailbackup-core`. Exit codes: 0 ok, 1 error, 2 partial.
use crate::progress::{with_progress, Failure};
use crate::{AddArgs, Cli, Command, EditArgs, ExportTo, Fields, ProfileCmd};
use mailbackup_core::service::{self, TargetKind};
use mailbackup_core::store::FsSqliteStore;
use mailbackup_core::{
    guess_imap, secret_key, Endpoint, Error, Profile, ProfileStore, Result, RunSummary, SecretRole,
    SecretStore, TargetSpec, TlsMode,
};
use std::io::{BufRead, IsTerminal};
use std::path::{Path, PathBuf};

const EXIT_OK: i32 = 0;
const EXIT_ERROR: i32 = 1;
const EXIT_PARTIAL: i32 = 2;

pub fn run(cli: Cli) -> Result<i32> {
    let cfg = config_dir(cli.config_dir)?;
    let profiles = ProfileStore::new(&cfg);
    let secrets = secret_store(&cfg);
    match cli.command {
        Command::Profile(p) => profile_cmd(p, &profiles, secrets.as_ref()),
        Command::Archive { profile } => {
            let p = profiles.load(&profile)?;
            let mut store = service::open_for_archive(&p)?;
            let (summary, failures) = run_archive(&p, secrets.as_ref(), &mut store)?;
            Ok(finish(None, &summary, &failures))
        }
        Command::Export { profile, to } => {
            let p = profiles.load(&profile)?;
            check_target(&p, secrets.as_ref(), Some(to))?;
            let mut store = service::open_existing(&p, true)?;
            let (summary, failures) = run_export(&p, secrets.as_ref(), &mut store, Some(to))?;
            Ok(finish(None, &summary, &failures))
        }
        Command::Migrate { profile } => {
            let p = profiles.load(&profile)?;
            p.since_epoch()?;
            // Fail fast, before any network or disk work.
            check_target(&p, secrets.as_ref(), None)?;
            let mut store = service::open_for_archive(&p)?;
            let (a, af) = run_archive(&p, secrets.as_ref(), &mut store)?;
            let ca = finish(Some("archive"), &a, &af);
            let (e, ef) = run_export(&p, secrets.as_ref(), &mut store, None)?;
            let ce = finish(Some("export"), &e, &ef);
            Ok(ca.max(ce))
        }
        Command::Verify { profile } => {
            let p = profiles.load(&profile)?;
            let r = service::verify(&p)?;
            println!("checked: {}", r.checked);
            println!("missing: {}", r.missing.len());
            println!("corrupt: {}", r.corrupt.len());
            for s in &r.missing {
                println!("  missing: {s}");
            }
            for s in &r.corrupt {
                println!("  corrupt: {s}");
            }
            Ok(if r.missing.is_empty() && r.corrupt.is_empty() {
                EXIT_OK
            } else {
                EXIT_ERROR
            })
        }
        Command::Reindex { profile } => {
            let p = profiles.load(&profile)?;
            let n = service::reindex(&p)?;
            println!("indexed {n} messages");
            Ok(EXIT_OK)
        }
        Command::Status { profile } => {
            let p = profiles.load(&profile)?;
            let st = service::status(&p)?;
            for f in &st.folders {
                println!("{}: {}", f.folder, f.messages);
            }
            println!("total: {} messages ({} unique)", st.total, st.unique);
            Ok(EXIT_OK)
        }
    }
}

fn remove_profile(name: &str, profiles: &ProfileStore, secrets: &dyn SecretStore) -> Result<()> {
    service::remove_profile(profiles, secrets, name)
}

fn kind(to: ExportTo) -> TargetKind {
    match to {
        ExportTo::Imap => TargetKind::Imap,
        ExportTo::Eml => TargetKind::Eml,
    }
}

/// Validates the export target (exists, kind matches `want`, password available) without
/// connecting or touching the archive.
fn check_target(p: &Profile, secrets: &dyn SecretStore, want: Option<ExportTo>) -> Result<()> {
    service::check_target(p, secrets, want.map(kind)).map_err(|e| hint(e, &p.name))
}

/// Adds the command that fixes a missing stored password to its error.
fn hint(e: Error, profile: &str) -> Error {
    match e {
        Error::Auth(m) if m.starts_with("no password stored") => {
            let target = if m.contains(":target'") {
                " --target"
            } else {
                ""
            };
            Error::Auth(format!(
                "{m}; set it with `profile set-password {profile}{target}`"
            ))
        }
        other => other,
    }
}

// ---- configuration & secrets ----

fn config_dir(flag: Option<PathBuf>) -> Result<PathBuf> {
    // `--config-dir` already falls back to MAILBACKUP_CONFIG_DIR through clap (flag wins).
    if let Some(d) = flag {
        return Ok(d);
    }
    dirs::config_dir()
        .map(|d| d.join("mailbackup"))
        .ok_or_else(|| {
            Error::Config("cannot determine the config directory; pass --config-dir".into())
        })
}

#[cfg(not(feature = "test-secrets"))]
fn secret_store(_cfg: &Path) -> Box<dyn SecretStore> {
    Box::new(mailbackup_core::KeyringSecrets)
}

#[cfg(feature = "test-secrets")]
fn secret_store(cfg: &Path) -> Box<dyn SecretStore> {
    Box::new(crate::file_secrets::FileSecrets::new(cfg))
}

/// Reads a password: hidden prompt on a terminal, otherwise (or with `from_stdin`) one stdin line.
fn read_password(prompt: &str, from_stdin: bool) -> Result<String> {
    let pw = if !from_stdin && std::io::stdin().is_terminal() {
        rpassword::prompt_password(prompt)?
    } else {
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    if pw.is_empty() {
        return Err(Error::Config("empty password; nothing was stored".into()));
    }
    Ok(pw)
}

fn tls_name(t: TlsMode) -> &'static str {
    match t {
        TlsMode::Implicit => "implicit",
        TlsMode::StartTls => "starttls",
    }
}

// ---- profile commands ----

fn profile_cmd(cmd: ProfileCmd, profiles: &ProfileStore, secrets: &dyn SecretStore) -> Result<i32> {
    match cmd {
        ProfileCmd::Add(a) => profile_add(a, profiles, secrets),
        ProfileCmd::List => {
            let names = profiles.list()?;
            if names.is_empty() {
                eprintln!("no profiles; create one with `profile add`");
            }
            for n in names {
                println!("{n}");
            }
            Ok(EXIT_OK)
        }
        ProfileCmd::Show { name } => {
            show(&profiles.load(&name)?);
            Ok(EXIT_OK)
        }
        ProfileCmd::Edit(e) => profile_edit(e, profiles),
        ProfileCmd::SetPassword {
            name,
            target,
            password_stdin,
        } => {
            let p = profiles.load(&name)?;
            let (role, who) = if target {
                match &p.target {
                    Some(TargetSpec::Imap(ep)) => (SecretRole::Target, ep.user.clone()),
                    _ => {
                        return Err(Error::Config(format!(
                            "profile '{name}' has no IMAP target"
                        )))
                    }
                }
            } else {
                (SecretRole::Source, p.source.user.clone())
            };
            let pw = read_password(&format!("Password for {who}: "), password_stdin)?;
            secrets.set(&secret_key(&name, role), &pw)?;
            println!("password stored for profile '{name}'");
            Ok(EXIT_OK)
        }
        ProfileCmd::Rm { name } => {
            remove_profile(&name, profiles, secrets)?;
            println!("profile '{name}' removed (the archive directory was not touched)");
            Ok(EXIT_OK)
        }
    }
}

fn show(p: &Profile) {
    println!("name: {}", p.name);
    println!("archive_dir: {}", p.archive_dir.display());
    let ep = |e: &Endpoint| {
        format!(
            "{}:{} tls={} user={}",
            e.host,
            e.port,
            tls_name(e.tls),
            e.user
        )
    };
    println!("source: {}", ep(&p.source));
    match &p.folders {
        Some(f) => println!("folders: {}", f.join(", ")),
        None => println!("folders: all"),
    }
    println!("since: {}", p.since.as_deref().unwrap_or("none"));
    match &p.target {
        None => println!("target: none"),
        Some(TargetSpec::Imap(e)) => println!("target: imap {}", ep(e)),
        Some(TargetSpec::EmlDir(d)) => println!("target: eml {}", d.display()),
    }
}

fn source_from(f: &Fields) -> Result<Option<Endpoint>> {
    if let Some(email) = &f.email {
        let mut ep = guess_imap(email)
            .ok_or_else(|| Error::Config(format!("'{email}' is not a valid e-mail address")))?;
        if let Some(u) = &f.user {
            ep.user = u.clone();
        }
        return Ok(Some(ep));
    }
    match &f.host {
        Some(host) => Ok(Some(Endpoint {
            host: host.clone(),
            port: f.port.unwrap_or(993),
            tls: f.tls.map(Into::into).unwrap_or(TlsMode::Implicit),
            user: f
                .user
                .clone()
                .ok_or_else(|| Error::Config("--user is required with --host".into()))?,
        })),
        None => Ok(None),
    }
}

/// Paths are stored absolute (without requiring them to exist), so running from another
/// working directory never silently uses a second archive.
fn absolute(p: &Path) -> Result<PathBuf> {
    Ok(std::path::absolute(p)?)
}

fn target_from(f: &Fields) -> Result<Option<TargetSpec>> {
    if let Some(d) = &f.target_eml {
        return Ok(Some(TargetSpec::EmlDir(absolute(d)?)));
    }
    if let Some(host) = &f.target_imap_host {
        return Ok(Some(TargetSpec::Imap(Endpoint {
            host: host.clone(),
            port: f.target_imap_port.unwrap_or(993),
            tls: f
                .target_imap_tls
                .map(Into::into)
                .unwrap_or(TlsMode::Implicit),
            user: f.target_imap_user.clone().ok_or_else(|| {
                Error::Config("--target-imap-user is required with --target-imap-host".into())
            })?,
        })));
    }
    if f.target_imap_port.is_some() || f.target_imap_tls.is_some() || f.target_imap_user.is_some() {
        return Err(Error::Config(
            "--target-imap-* options need --target-imap-host".into(),
        ));
    }
    Ok(None)
}

fn profile_add(a: AddArgs, profiles: &ProfileStore, secrets: &dyn SecretStore) -> Result<i32> {
    // Name checks first, before the other fields are even parsed.
    service::validate_new_profile(
        profiles,
        &Profile {
            name: a.name.clone(),
            archive_dir: PathBuf::new(),
            source: Endpoint {
                host: String::new(),
                port: 0,
                tls: TlsMode::Implicit,
                user: String::new(),
            },
            folders: None,
            since: None,
            target: None,
        },
    )?;
    let f = &a.fields;
    let source = source_from(f)?.ok_or_else(|| {
        Error::Config("give either --email or --host/--user for the source".into())
    })?;
    let archive_dir = absolute(
        f.archive_dir
            .as_deref()
            .ok_or_else(|| Error::Config("--archive-dir is required".into()))?,
    )?;
    let profile = Profile {
        name: a.name.clone(),
        archive_dir,
        source,
        folders: if f.folders.is_empty() {
            None
        } else {
            Some(f.folders.clone())
        },
        since: f.since.clone(),
        target: target_from(f)?,
    };
    service::validate_new_profile(profiles, &profile)?;

    // Read every password before touching any state.
    let src_pw = read_password(
        &format!("Password for {}: ", profile.source.user),
        a.password_stdin,
    )?;
    let tgt_pw = match &profile.target {
        Some(TargetSpec::Imap(e)) => Some(read_password(
            &format!("Password for target {}: ", e.user),
            a.password_stdin,
        )?),
        _ => None,
    };
    service::create_profile(profiles, secrets, &profile, &src_pw, tgt_pw.as_deref())?;
    println!("profile '{}' saved", a.name);
    Ok(EXIT_OK)
}

fn profile_edit(e: EditArgs, profiles: &ProfileStore) -> Result<i32> {
    let mut p = profiles.load(&e.name)?;
    let f = &e.fields;
    if f.email.is_some() || f.host.is_some() {
        p.source = source_from(f)?.expect("host or email present");
    } else {
        if let Some(port) = f.port {
            p.source.port = port;
        }
        if let Some(t) = f.tls {
            p.source.tls = t.into();
        }
        if let Some(u) = &f.user {
            p.source.user = u.clone();
        }
    }
    if let Some(d) = &f.archive_dir {
        p.archive_dir = absolute(d)?;
    }
    if e.clear_folders {
        p.folders = None;
    }
    if !f.folders.is_empty() {
        p.folders = Some(f.folders.clone());
    }
    if e.clear_since {
        p.since = None;
    }
    if let Some(s) = &f.since {
        p.since = Some(s.clone());
    }
    p.since_epoch()?;
    if e.clear_target {
        p.target = None;
    }
    let mut needs_target_pw = false;
    if f.target_eml.is_some() || f.target_imap_host.is_some() {
        p.target = target_from(f)?;
        needs_target_pw = matches!(p.target, Some(TargetSpec::Imap(_)));
    } else if f.target_imap_port.is_some()
        || f.target_imap_tls.is_some()
        || f.target_imap_user.is_some()
    {
        match &mut p.target {
            Some(TargetSpec::Imap(t)) => {
                if let Some(port) = f.target_imap_port {
                    t.port = port;
                }
                if let Some(tls) = f.target_imap_tls {
                    t.tls = tls.into();
                }
                if let Some(u) = &f.target_imap_user {
                    t.user = u.clone();
                }
            }
            _ => {
                return Err(Error::Config(
                    "--target-imap-* options need --target-imap-host".into(),
                ))
            }
        }
    }
    let reset = service::update_profile(profiles, &p)?;
    println!("profile '{}' updated", p.name);
    if reset {
        println!("the 'since' filter changed: the next `archive` will also fetch older mail it does not have yet");
    }
    if needs_target_pw {
        println!(
            "set the target password with `profile set-password {} --target`",
            p.name
        );
    }
    Ok(EXIT_OK)
}

// ---- engines ----

type Outcome = (RunSummary, Vec<Failure>);

fn run_archive(
    p: &Profile,
    secrets: &dyn SecretStore,
    store: &mut FsSqliteStore,
) -> Result<Outcome> {
    let mut source = service::source_for(p, secrets).map_err(|e| hint(e, &p.name))?;
    let (res, failures) = with_progress("archive", |tx| {
        service::archive_with(p, &mut source, &mut *store, None, &tx)
    });
    match res {
        Ok(s) => Ok((s, failures)),
        Err(e) => {
            report_failures(&failures);
            Err(e)
        }
    }
}

fn run_export(
    p: &Profile,
    secrets: &dyn SecretStore,
    store: &mut FsSqliteStore,
    want: Option<ExportTo>,
) -> Result<Outcome> {
    check_target(p, secrets, want)?;
    let target = service::target_for(p, secrets).map_err(|e| hint(e, &p.name))?;
    let (res, failures) = with_progress("export", |tx| {
        service::export_with(p, &mut *store, target, None, &tx)
    });
    match res {
        Ok(s) => Ok((s, failures)),
        Err(e) => {
            report_failures(&failures);
            Err(e)
        }
    }
}

fn report_failures(failures: &[Failure]) {
    for f in failures {
        match f.uid {
            Some(uid) => eprintln!("failed: {} uid {uid}: {}", f.folder, f.reason),
            None => eprintln!("failed: folder {}: {}", f.folder, f.reason),
        }
    }
}

/// Prints the summary and returns the exit code (2 when any message failed).
fn finish(heading: Option<&str>, s: &RunSummary, failures: &[Failure]) -> i32 {
    if let Some(h) = heading {
        println!("{h}:");
    }
    println!("added: {}", s.added);
    println!("duplicates: {}", s.duplicates);
    println!("failed: {}", s.failed);
    report_failures(failures);
    if s.failed > 0 {
        EXIT_PARTIAL
    } else {
        EXIT_OK
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mailbackup_core::MemorySecrets;

    struct FailingDelete(MemorySecrets);
    impl SecretStore for FailingDelete {
        fn set(&self, k: &str, s: &str) -> Result<()> {
            self.0.set(k, s)
        }
        fn get(&self, k: &str) -> Result<String> {
            self.0.get(k)
        }
        fn delete(&self, _k: &str) -> Result<()> {
            Err(Error::Config("store is locked".into()))
        }
    }

    #[test]
    fn folder_failure_is_listed_and_makes_the_run_partial() {
        let (summary, failures) = with_progress("archive", |tx| {
            tx.send(mailbackup_core::Event::FolderFailed {
                folder: "Nope".into(),
                reason: "folder not found".into(),
            })
            .unwrap();
            RunSummary {
                added: 3,
                duplicates: 0,
                failed: 1,
            }
        });
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].folder, "Nope");
        assert_eq!(failures[0].uid, None);
        assert_eq!(failures[0].reason, "folder not found");
        assert_eq!(finish(None, &summary, &failures), EXIT_PARTIAL);
    }

    #[test]
    fn rm_keeps_profile_when_secret_delete_fails() {
        let d = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::new(d.path());
        profiles
            .save(&Profile {
                name: "w".into(),
                archive_dir: d.path().join("a"),
                source: Endpoint {
                    host: "h".into(),
                    port: 993,
                    tls: TlsMode::Implicit,
                    user: "u".into(),
                },
                folders: None,
                since: None,
                target: None,
            })
            .unwrap();
        let secrets = FailingDelete(MemorySecrets::new());
        assert!(remove_profile("w", &profiles, &secrets).is_err());
        assert!(profiles.load("w").is_ok());
    }
}
