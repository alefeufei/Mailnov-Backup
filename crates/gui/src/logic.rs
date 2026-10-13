//! Window-free logic behind the Tauri commands: everything here takes an [`AppState`] and plain
//! values, so it is unit-tested without a WebView, a real mail server or the OS credential store.
use mailbackup_core::cancel::{is_cancelled, is_cancelled_reason};
use mailbackup_core::service::{self, ExitClass, Failure, StatusReport};
use mailbackup_core::store::FsSqliteStore;
use mailbackup_core::{
    guess_imap, secret_key, Endpoint, Error, Event, MailSource, MailTarget, Profile, ProfileStore,
    RunSummary, SecretRole, SecretStore, TargetSpec, TlsMode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Minimum time between two progress events sent to the window.
pub const THROTTLE: Duration = Duration::from_millis(100);
/// At most this many failures are kept in memory and sent with the final result.
pub const MAX_FAILURES: usize = 500;

// ---- backend (real IMAP in the app, fakes in tests) ----

/// Builds connections. The app uses [`ImapBackend`]; tests inject fakes.
pub trait Backend: Send + Sync {
    fn source(&self, p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailSource>, Error>;
    fn target(&self, p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailTarget>, Error>;
    fn test_connection(&self, ep: &Endpoint, password: String) -> Result<Vec<String>, Error>;
}

pub struct ImapBackend;

impl Backend for ImapBackend {
    fn source(&self, p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailSource>, Error> {
        Ok(Box::new(service::source_for(p, secrets)?))
    }
    fn target(&self, p: &Profile, secrets: &dyn SecretStore) -> Result<Box<dyn MailTarget>, Error> {
        service::target_for(p, secrets)
    }
    fn test_connection(&self, ep: &Endpoint, password: String) -> Result<Vec<String>, Error> {
        service::test_connection(ep, password)
    }
}

// ---- state ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Archive,
    Export,
    Migrate,
}

impl Op {
    fn label(self) -> &'static str {
        match self {
            Op::Archive => "archive",
            Op::Export => "export",
            Op::Migrate => "migrate",
        }
    }
}

struct Running {
    op: Op,
    cancel: Arc<AtomicBool>,
}

pub struct AppState {
    pub config_dir: PathBuf,
    pub profiles: ProfileStore,
    pub secrets: Box<dyn SecretStore + Send + Sync>,
    pub backend: Box<dyn Backend>,
    running: Mutex<HashMap<String, Running>>,
}

impl AppState {
    pub fn new(
        config_dir: PathBuf,
        secrets: Box<dyn SecretStore + Send + Sync>,
        backend: Box<dyn Backend>,
    ) -> Self {
        Self {
            profiles: ProfileStore::new(&config_dir),
            config_dir,
            secrets,
            backend,
            running: Mutex::new(HashMap::new()),
        }
    }

    fn running(&self) -> std::sync::MutexGuard<'_, HashMap<String, Running>> {
        // A panic while holding the lock leaves a plain map behind: keep using it.
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn running_op(&self, name: &str) -> Option<Op> {
        self.running().get(name).map(|r| r.op)
    }

    fn runs_dir(&self) -> PathBuf {
        self.config_dir.join("gui").join("runs")
    }
}

/// `MAILBACKUP_CONFIG_DIR` (as for the CLI) or the user's config directory + `mailbackup`.
pub fn default_config_dir() -> Result<PathBuf, String> {
    if let Some(d) = std::env::var_os("MAILBACKUP_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Ok(PathBuf::from(d));
    }
    dirs::config_dir()
        .map(|d| d.join("mailbackup"))
        .ok_or_else(|| "Não foi possível descobrir a pasta de configuração do usuário.".into())
}

// ---- errors in plain language ----

/// A user-facing (Portuguese) explanation of `e`, followed by the technical detail. Core errors
/// never contain passwords.
pub fn humanize(e: &Error) -> String {
    if is_cancelled(e) {
        return "Cancelado pelo usuário.".into();
    }
    let detail = e.to_string();
    let lower = detail.to_lowercase();
    let what = match e {
        Error::Auth(m) if m.starts_with("no password stored") => {
            return "A senha desta conta não está guardada no Gerenciador de Credenciais do \
                    Windows. Edite a conta e informe a senha de novo."
                .into()
        }
        Error::Auth(m) if m.contains("characters IMAP cannot send") => {
            "O usuário ou a senha contém caracteres que o protocolo IMAP não consegue enviar."
        }
        Error::Auth(_) => {
            "O servidor recusou o login: usuário ou senha incorretos, ou o acesso IMAP está \
             desativado nesta conta."
        }
        Error::Connection(_) if lower.contains("certificate") || lower.contains("certificado") => {
            "O certificado de segurança (TLS) do servidor não é válido para este endereço. \
             Confira o nome do servidor; a verificação do certificado nunca é desativada."
        }
        Error::Connection(_) => {
            "Não foi possível falar com o servidor. Confira o endereço, a porta, o tipo de \
             segurança e a sua conexão com a internet."
        }
        Error::Imap(_) if lower.contains("starttls") || lower.contains("tls") => {
            "O servidor não oferece a conexão segura escolhida. Tente a outra opção de \
             segurança (SSL/TLS na porta 993 ou STARTTLS na porta 143)."
        }
        Error::Imap(_) => "O servidor de e-mail respondeu com um erro.",
        Error::Config(_) => "Configuração inválida.",
        Error::Io(_) => "Erro ao ler ou gravar arquivos no disco.",
        Error::Db(_) => "Erro no índice do backup (index.db).",
        Error::Corrupt(_) => "Há dados corrompidos no backup.",
        Error::FolderGone(_) => "A pasta deixou de existir no servidor durante a operação.",
    };
    format!("{what}\nDetalhe técnico: {detail}")
}

fn err(e: Error) -> String {
    humanize(&e)
}

fn cfg(msg: impl Into<String>) -> String {
    msg.into()
}

// ---- profiles ----

fn tls_from(s: &str) -> Result<TlsMode, String> {
    match s {
        "implicit" | "ssl" => Ok(TlsMode::Implicit),
        "starttls" => Ok(TlsMode::StartTls),
        other => Err(cfg(format!("Tipo de segurança desconhecido: {other}"))),
    }
}

fn tls_name(t: TlsMode) -> &'static str {
    match t {
        TlsMode::Implicit => "implicit",
        TlsMode::StartTls => "starttls",
    }
}

/// Connection fields as the window sends them.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EndpointInput {
    pub host: String,
    pub port: u16,
    pub tls: String,
    pub user: String,
}

impl EndpointInput {
    fn to_endpoint(&self) -> Result<Endpoint, String> {
        let host = self.host.trim();
        let user = self.user.trim();
        if host.is_empty() {
            return Err(cfg("Informe o endereço do servidor."));
        }
        if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(cfg("O endereço do servidor não pode conter espaços."));
        }
        if user.is_empty() {
            return Err(cfg("Informe o usuário."));
        }
        if self.port == 0 {
            return Err(cfg("Informe uma porta válida (1 a 65535)."));
        }
        Ok(Endpoint {
            host: host.to_string(),
            port: self.port,
            tls: tls_from(&self.tls)?,
            user: user.to_string(),
        })
    }

    fn from_endpoint(e: &Endpoint) -> Self {
        Self {
            host: e.host.clone(),
            port: e.port,
            tls: tls_name(e.tls).into(),
            user: e.user.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Detected {
    pub endpoint: EndpointInput,
    /// Shown next to the password field (e.g. "use an app password").
    pub hint: Option<String>,
    pub suggested_name: String,
    pub suggested_dir: String,
}

fn provider_hint(host: &str) -> Option<String> {
    let h = host.to_ascii_lowercase();
    if h.ends_with("gmail.com") || h.ends_with("googlemail.com") {
        Some(
            "Gmail: ative o IMAP e use uma senha de app (Conta Google > Segurança > Senhas \
             de app). A senha normal não funciona."
                .into(),
        )
    } else if h.contains("office365") || h.contains("outlook") {
        Some(
            "Microsoft 365 / Outlook.com: use uma senha de app (ou peça ao administrador para \
             liberar IMAP com senha). Login com OAuth ainda não é suportado."
                .into(),
        )
    } else if h.contains("yahoo") || h.contains("mail.me.com") {
        Some("Este provedor exige uma senha de app gerada nas configurações da conta.".into())
    } else {
        None
    }
}

/// A valid profile name derived from `email`, unique among `existing` (ignoring case).
pub fn suggest_name(email: &str, existing: &[String]) -> String {
    let (local, domain) = email.split_once('@').unwrap_or((email, ""));
    let first = domain.split('.').next().unwrap_or("");
    let raw = if first.is_empty() {
        local.to_string()
    } else {
        format!("{local}-{first}")
    };
    let mut base: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    base = base.trim_matches('.').to_string();
    base.truncate(56);
    let base = base.trim_end_matches('.').to_string();
    let base = if mailbackup_core::profile::validate_name(&base).is_ok() {
        base
    } else {
        "conta".to_string()
    };
    let taken = |n: &str| existing.iter().any(|e| e.eq_ignore_ascii_case(n));
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|i| format!("{base}-{i}"))
        .find(|n| !taken(n))
        .expect("an unused suffix exists")
}

/// `Documents\MailBackup\<name>` (or the home directory when there is no Documents folder).
pub fn suggest_dir(name: &str) -> String {
    let base = dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("MailBackup").join(name).display().to_string()
}

pub fn detect_server(state: &AppState, email: &str) -> Result<Detected, String> {
    let email = email.trim();
    let ep = guess_imap(email).ok_or_else(|| cfg("Digite um endereço de e-mail válido."))?;
    let existing = state.profiles.list().map_err(err)?;
    let name = suggest_name(email, &existing);
    Ok(Detected {
        hint: provider_hint(&ep.host),
        endpoint: EndpointInput::from_endpoint(&ep),
        suggested_dir: suggest_dir(&name),
        suggested_name: name,
    })
}

pub fn test_connection(
    state: &AppState,
    ep: &EndpointInput,
    password: String,
) -> Result<Vec<String>, String> {
    let ep = ep.to_endpoint()?;
    if password.is_empty() {
        return Err(cfg("Informe a senha."));
    }
    let hint = provider_hint(&ep.host);
    state
        .backend
        .test_connection(&ep, password)
        .map_err(|e| match (&e, hint) {
            (Error::Auth(_), Some(h)) => format!("{}\n{h}", humanize(&e)),
            _ => humanize(&e),
        })
}

/// What the account wizard sends to create (or edit) a profile.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileInput {
    pub name: String,
    pub source: EndpointInput,
    /// `None` keeps the stored password (edit only).
    pub password: Option<String>,
    /// `None` = every folder.
    pub folders: Option<Vec<String>>,
    /// `YYYY-MM-DD` or empty.
    pub since: Option<String>,
    pub archive_dir: String,
    pub is_new: bool,
}

fn since_of(s: &Option<String>) -> Option<String> {
    s.as_ref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn abs_dir(dir: &str, what: &str) -> Result<PathBuf, String> {
    let d = dir.trim();
    if d.is_empty() {
        return Err(cfg(format!("Escolha {what}.")));
    }
    let p = PathBuf::from(d);
    if !p.is_absolute() {
        return Err(cfg(format!(
            "Use um caminho completo para {what} (por exemplo C:\\Backups\\conta)."
        )));
    }
    std::path::absolute(&p).map_err(|e| err(Error::Io(e)))
}

pub fn save_profile(state: &AppState, input: ProfileInput) -> Result<(), String> {
    let name = input.name.trim().to_string();
    mailbackup_core::profile::validate_name(&name).map_err(|_| {
        cfg(
            "Nome inválido: use de 1 a 64 letras sem acento, números, '.', '_' ou '-' \
             (sem espaços, sem começar ou terminar com '.').",
        )
    })?;
    if state.running_op(&name).is_some() {
        return Err(cfg(
            "Aguarde a operação em andamento terminar antes de alterar a conta.",
        ));
    }
    let source = input.source.to_endpoint()?;
    let folders = input
        .folders
        .map(|f| {
            f.into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .filter(|f| !f.is_empty());
    let archive_dir = abs_dir(&input.archive_dir, "a pasta do backup")?;
    let since = since_of(&input.since);
    if input.is_new {
        let p = Profile {
            name: name.clone(),
            archive_dir,
            source,
            folders,
            since,
            target: None,
        };
        if p.since_epoch().is_err() {
            return Err(cfg("Data inválida em \"arquivar apenas e-mails desde\"."));
        }
        let pw = input.password.unwrap_or_default();
        if pw.is_empty() {
            return Err(cfg("Informe a senha."));
        }
        if state.profiles.list().map_err(err)?.contains(&name) {
            return Err(cfg(format!("Já existe uma conta chamada \"{name}\".")));
        }
        if let Some(other) = state.profiles.case_conflict(&name).map_err(err)? {
            return Err(cfg(format!(
                "Já existe uma conta chamada \"{other}\" (nomes não diferenciam maiúsculas \
                 de minúsculas). Escolha outro nome."
            )));
        }
        service::create_profile(&state.profiles, state.secrets.as_ref(), &p, &pw, None).map_err(err)
    } else {
        let mut p = state.profiles.load(&name).map_err(err)?;
        p.source = source;
        p.folders = folders;
        p.since = since;
        p.archive_dir = archive_dir;
        if p.since_epoch().is_err() {
            return Err(cfg("Data inválida em \"arquivar apenas e-mails desde\"."));
        }
        if let Some(pw) = input.password.filter(|p| !p.is_empty()) {
            state
                .secrets
                .set(&secret_key(&name, SecretRole::Source), &pw)
                .map_err(err)?;
        }
        service::update_profile(&state.profiles, &p)
            .map(|_| ())
            .map_err(err)
    }
}

/// Export destination as the window sends it.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetInput {
    /// "eml", "imap" or "none".
    pub kind: String,
    pub dir: Option<String>,
    pub endpoint: Option<EndpointInput>,
    /// IMAP only; `None`/empty keeps the stored one (required when none is stored).
    pub password: Option<String>,
}

pub fn set_target(state: &AppState, name: &str, input: TargetInput) -> Result<(), String> {
    if state.running_op(name).is_some() {
        return Err(cfg(
            "Aguarde a operação em andamento terminar antes de alterar a conta.",
        ));
    }
    let mut p = state.profiles.load(name).map_err(err)?;
    let key = secret_key(name, SecretRole::Target);
    match input.kind.as_str() {
        "none" => {
            p.target = None;
            state.profiles.save(&p).map_err(err)?;
            state.secrets.delete(&key).map_err(err)
        }
        "eml" => {
            let dir = abs_dir(input.dir.as_deref().unwrap_or(""), "a pasta de destino")?;
            if dir == p.archive_dir || dir.starts_with(&p.archive_dir) {
                return Err(cfg(
                    "A pasta de destino não pode ficar dentro da pasta do backup.",
                ));
            }
            p.target = Some(TargetSpec::EmlDir(dir));
            state.profiles.save(&p).map_err(err)?;
            state.secrets.delete(&key).map_err(err)
        }
        "imap" => {
            let ep = input
                .endpoint
                .as_ref()
                .ok_or_else(|| cfg("Informe o servidor de destino."))?
                .to_endpoint()?;
            match input.password.filter(|p| !p.is_empty()) {
                Some(pw) => state.secrets.set(&key, &pw).map_err(err)?,
                None => {
                    if state.secrets.get(&key).is_err() {
                        return Err(cfg("Informe a senha da conta de destino."));
                    }
                }
            }
            p.target = Some(TargetSpec::Imap(ep));
            state.profiles.save(&p).map_err(err)
        }
        other => Err(cfg(format!("Tipo de destino desconhecido: {other}"))),
    }
}

pub fn delete_profile(state: &AppState, name: &str) -> Result<(), String> {
    if state.running_op(name).is_some() {
        return Err(cfg(
            "Cancele a operação em andamento antes de remover a conta.",
        ));
    }
    service::remove_profile(&state.profiles, state.secrets.as_ref(), name).map_err(err)?;
    let _ = std::fs::remove_file(run_file(state, name));
    Ok(())
}

// ---- last-run records (GUI only; never inside the archive) ----

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub op: Op,
    pub class: String,
    pub finished_at: i64,
    pub added: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RunHistory {
    pub last_run: Option<RunRecord>,
    /// Last archive that finished (fully or with failed items).
    pub last_archive: Option<RunRecord>,
}

fn run_file(state: &AppState, name: &str) -> PathBuf {
    state.runs_dir().join(format!("{name}.json"))
}

pub fn load_history(state: &AppState, name: &str) -> RunHistory {
    std::fs::read_to_string(run_file(state, name))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_history(state: &AppState, name: &str, h: &RunHistory) {
    // Best effort: the history only feeds the "last archive" line and the status dot.
    let _ = std::fs::create_dir_all(state.runs_dir());
    if let Ok(text) = serde_json::to_string_pretty(h) {
        let _ = std::fs::write(run_file(state, name), text);
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---- listing & status ----

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileSummary {
    pub name: String,
    pub source: EndpointInput,
    pub archive_dir: String,
    pub folders: Option<Vec<String>>,
    pub since: Option<String>,
    /// "none", "eml" or "imap".
    pub target_kind: String,
    pub target_dir: Option<String>,
    pub target_endpoint: Option<EndpointInput>,
    pub archive_exists: bool,
    /// Distinct archived messages (absent while an operation runs or when unreadable).
    pub messages: Option<usize>,
    pub folder_count: Option<usize>,
    pub last_archive_at: Option<i64>,
    pub last_run: Option<RunRecord>,
    pub running: Option<Op>,
    /// Set when the profile file could not be read.
    pub error: Option<String>,
}

fn summary_of(state: &AppState, name: &str) -> ProfileSummary {
    let history = load_history(state, name);
    let running = state.running_op(name);
    let p = match state.profiles.load(name) {
        Ok(p) => p,
        Err(e) => {
            return ProfileSummary {
                name: name.into(),
                source: EndpointInput {
                    host: String::new(),
                    port: 0,
                    tls: "implicit".into(),
                    user: String::new(),
                },
                archive_dir: String::new(),
                folders: None,
                since: None,
                target_kind: "none".into(),
                target_dir: None,
                target_endpoint: None,
                archive_exists: false,
                messages: None,
                folder_count: None,
                last_archive_at: None,
                last_run: history.last_run,
                running,
                error: Some(humanize(&e)),
            }
        }
    };
    let exists = service::archive_exists(&p.archive_dir);
    let status = if exists && running.is_none() {
        service::status(&p).ok()
    } else {
        None
    };
    let (target_kind, target_dir, target_endpoint) = match &p.target {
        None => ("none", None, None),
        Some(TargetSpec::EmlDir(d)) => ("eml", Some(d.display().to_string()), None),
        Some(TargetSpec::Imap(e)) => ("imap", None, Some(EndpointInput::from_endpoint(e))),
    };
    ProfileSummary {
        name: p.name.clone(),
        source: EndpointInput::from_endpoint(&p.source),
        archive_dir: p.archive_dir.display().to_string(),
        folders: p.folders.clone(),
        since: p.since.clone(),
        target_kind: target_kind.into(),
        target_dir,
        target_endpoint,
        archive_exists: exists,
        messages: status.as_ref().map(|s| s.unique),
        folder_count: status.as_ref().map(|s| s.folders.len()),
        last_archive_at: history.last_archive.as_ref().map(|r| r.finished_at),
        last_run: history.last_run,
        running,
        error: None,
    }
}

pub fn list_profiles(state: &AppState) -> Result<Vec<ProfileSummary>, String> {
    let names = state.profiles.list().map_err(err)?;
    Ok(names.iter().map(|n| summary_of(state, n)).collect())
}

pub fn get_status(state: &AppState, name: &str) -> Result<StatusReport, String> {
    let p = state.profiles.load(name).map_err(err)?;
    if !service::archive_exists(&p.archive_dir) {
        return Ok(StatusReport::default());
    }
    if state.running_op(name).is_some() {
        return Err(cfg(
            "Operação em andamento; a contagem aparece quando ela terminar.",
        ));
    }
    service::status(&p).map_err(err)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifyResult {
    pub checked: usize,
    pub missing: Vec<String>,
    pub corrupt: Vec<String>,
}

fn idle_profile(state: &AppState, name: &str) -> Result<Profile, String> {
    if state.running_op(name).is_some() {
        return Err(cfg("Já existe uma operação em andamento para esta conta."));
    }
    let p = state.profiles.load(name).map_err(err)?;
    if !service::archive_exists(&p.archive_dir) {
        return Err(cfg(
            "Ainda não há backup desta conta. Clique em \"Arquivar agora\" primeiro.",
        ));
    }
    Ok(p)
}

pub fn run_verify(state: &AppState, name: &str) -> Result<VerifyResult, String> {
    let p = idle_profile(state, name)?;
    let r = service::verify(&p).map_err(reindex_hint)?;
    Ok(VerifyResult {
        checked: r.checked,
        missing: r.missing,
        corrupt: r.corrupt,
    })
}

fn reindex_hint(e: Error) -> String {
    match &e {
        Error::Config(m) if m.contains("index") && m.contains("missing") => {
            "O índice do backup (index.db) não existe. Use \"Reconstruir índice\".".into()
        }
        _ => humanize(&e),
    }
}

pub fn run_reindex(state: &AppState, name: &str) -> Result<usize, String> {
    let p = idle_profile(state, name)?;
    service::reindex(&p).map_err(err)
}

// ---- long operations ----

/// Live progress pushed to the window (`progress` event).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub profile: String,
    pub op: Op,
    /// "connect", "archive" or "export".
    pub phase: String,
    pub folder: Option<String>,
    pub done: usize,
    pub total: usize,
    /// Messages processed (archived or already present / exported) in this phase.
    pub processed: usize,
    pub failed: usize,
    pub folders_done: usize,
    /// Failures since the previous progress event.
    pub new_failures: Vec<Failure>,
    pub message: Option<String>,
    pub cancelling: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PhaseResult {
    pub phase: String,
    pub class: String,
    pub added: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub error: Option<String>,
}

/// Final result (`finished` event).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Finished {
    pub profile: String,
    pub op: Op,
    /// "ok", "partial", "error" or "cancelled".
    pub class: String,
    pub added: usize,
    pub duplicates: usize,
    pub failed: usize,
    pub failures: Vec<Failure>,
    pub failures_total: usize,
    pub error: Option<String>,
    pub phases: Vec<PhaseResult>,
    pub finished_at: i64,
}

/// Where progress goes: the window in the app, a recorder in tests.
pub trait Sink: Send + Sync {
    fn progress(&self, p: &Progress);
    fn finished(&self, f: &Finished);
}

/// Folds engine events into the progress shown to the user.
pub struct Tracker {
    pub state: Progress,
    pub failures: Vec<Failure>,
    pub failures_total: usize,
    pending: Vec<Failure>,
}

impl Tracker {
    pub fn new(profile: &str, op: Op, phase: &str) -> Self {
        Self {
            state: Progress {
                profile: profile.into(),
                op,
                phase: phase.into(),
                folder: None,
                done: 0,
                total: 0,
                processed: 0,
                failed: 0,
                folders_done: 0,
                new_failures: vec![],
                message: None,
                cancelling: false,
            },
            failures: vec![],
            failures_total: 0,
            pending: vec![],
        }
    }

    pub fn phase(&mut self, phase: &str, message: Option<String>) {
        let s = &mut self.state;
        s.phase = phase.into();
        s.folder = None;
        s.done = 0;
        s.total = 0;
        s.processed = 0;
        s.folders_done = 0;
        s.message = message;
    }

    pub fn on_event(&mut self, e: &Event) {
        let s = &mut self.state;
        match e {
            Event::FolderStarted { folder, total } => {
                if s.folder.is_some() {
                    s.folders_done += 1;
                }
                s.folder = Some(folder.clone());
                s.total = *total;
                s.done = 0;
                s.message = None;
            }
            Event::MessageArchived { .. } => {
                s.done += 1;
                s.processed += 1;
            }
            Event::MessageFailed { .. } | Event::FolderFailed { .. } => {
                let f = Failure::from_event(e).expect("a failure event");
                // A cancellation is not a failure of the message.
                if is_cancelled_reason(&f.reason) {
                    return;
                }
                if matches!(e, Event::MessageFailed { .. }) {
                    s.done += 1;
                }
                s.failed += 1;
                self.failures_total += 1;
                if self.failures.len() < MAX_FAILURES {
                    self.failures.push(f.clone());
                }
                if self.pending.len() < MAX_FAILURES {
                    self.pending.push(f);
                }
            }
            Event::Finished(_) => {
                if s.folder.is_some() {
                    s.folders_done += 1;
                }
            }
        }
    }

    /// The state to send now (takes the failures accumulated since the last snapshot).
    pub fn snapshot(&mut self) -> Progress {
        let mut p = self.state.clone();
        p.new_failures = std::mem::take(&mut self.pending);
        p
    }
}

/// Runs `work` on this thread with an event channel while a helper thread forwards throttled
/// progress to `sink` (at most one event per [`THROTTLE`]; the last state is always sent).
fn with_events<T>(
    tracker: &mut Tracker,
    sink: &dyn Sink,
    cancel: &AtomicBool,
    work: impl FnOnce(Sender<Event>) -> T,
) -> T {
    let (tx, rx) = channel::<Event>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            let mut last: Option<Instant> = None;
            let mut dirty = true;
            loop {
                match rx.recv_timeout(THROTTLE) {
                    Ok(e) => {
                        tracker.on_event(&e);
                        dirty = true;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                let cancelling = cancel.load(Ordering::SeqCst);
                if cancelling != tracker.state.cancelling {
                    tracker.state.cancelling = cancelling;
                    dirty = true;
                }
                if dirty && last.is_none_or(|t| t.elapsed() >= THROTTLE) {
                    sink.progress(&tracker.snapshot());
                    last = Some(Instant::now());
                    dirty = false;
                }
            }
            tracker.state.cancelling = cancel.load(Ordering::SeqCst);
            sink.progress(&tracker.snapshot());
        });
        // `tx` is dropped when `work` returns, which ends the forwarding loop.
        work(tx)
    })
}

fn class_name(c: ExitClass) -> &'static str {
    match c {
        ExitClass::Ok => "ok",
        ExitClass::Partial => "partial",
        ExitClass::Error => "error",
    }
}

fn phase_result(phase: &str, r: &Result<RunSummary, Error>, cancel: &AtomicBool) -> PhaseResult {
    let cancelled = cancel.load(Ordering::SeqCst);
    match r {
        Ok(s) => PhaseResult {
            phase: phase.into(),
            class: if cancelled && s.failed > 0 {
                "cancelled".into()
            } else {
                class_name(ExitClass::of(r)).into()
            },
            added: s.added,
            duplicates: s.duplicates,
            failed: s.failed,
            error: None,
        },
        Err(e) => PhaseResult {
            phase: phase.into(),
            class: if cancelled || is_cancelled(e) {
                "cancelled".into()
            } else {
                "error".into()
            },
            added: 0,
            duplicates: 0,
            failed: 0,
            error: Some(humanize(e)),
        },
    }
}

/// Checks done before the worker starts, so an obvious problem is reported at once.
fn preflight(state: &AppState, p: &Profile, op: Op) -> Result<(), String> {
    if p.since_epoch().is_err() {
        return Err(cfg(
            "A data \"arquivar apenas e-mails desde\" desta conta é inválida.",
        ));
    }
    let need_source = matches!(op, Op::Archive | Op::Migrate);
    if need_source {
        service::password(state.secrets.as_ref(), &p.name, SecretRole::Source).map_err(err)?;
    }
    if matches!(op, Op::Export | Op::Migrate) {
        if p.target.is_none() {
            return Err(cfg(
                "Esta conta ainda não tem destino de exportação. Use \"Exportar…\" para escolher \
                 uma pasta de .eml ou outro servidor IMAP.",
            ));
        }
        service::check_target(p, state.secrets.as_ref(), None).map_err(err)?;
    }
    if op == Op::Export {
        if !service::archive_exists(&p.archive_dir) {
            return Err(cfg(
                "Ainda não há backup desta conta para exportar. Clique em \"Arquivar agora\" \
                 primeiro.",
            ));
        }
        if !p.archive_dir.join("index.db").is_file() {
            return Err(cfg(
                "O índice do backup (index.db) não existe. Use \"Reconstruir índice\" antes.",
            ));
        }
    }
    Ok(())
}

/// Starts `op` for profile `name` on a worker thread. Returns at once; progress and the final
/// result go to `sink`. A second operation on the same profile is refused.
pub fn start(state: Arc<AppState>, name: &str, op: Op, sink: Arc<dyn Sink>) -> Result<(), String> {
    let p = state.profiles.load(name).map_err(err)?;
    preflight(&state, &p, op)?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut running = state.running();
        if let Some(r) = running.get(name) {
            let what = match r.op {
                Op::Archive => "um arquivamento",
                Op::Export => "uma exportação",
                Op::Migrate => "uma migração",
            };
            return Err(cfg(format!(
                "Já existe {what} em andamento para esta conta. Aguarde ou cancele."
            )));
        }
        running.insert(
            name.to_string(),
            Running {
                op,
                cancel: cancel.clone(),
            },
        );
    }
    let name = name.to_string();
    let spawned = std::thread::Builder::new()
        .name(format!("{}-{name}", op.label()))
        .spawn({
            let state = state.clone();
            let name = name.clone();
            move || {
                let finished = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute(&state, &p, op, &cancel, sink.as_ref())
                }))
                .unwrap_or_else(|_| Finished {
                    profile: name.clone(),
                    op,
                    class: "error".into(),
                    added: 0,
                    duplicates: 0,
                    failed: 0,
                    failures: vec![],
                    failures_total: 0,
                    error: Some("Erro interno inesperado; a operação parou.".into()),
                    phases: vec![],
                    finished_at: now(),
                });
                record(&state, &name, &finished);
                state.running().remove(&name);
                sink.finished(&finished);
            }
        });
    if let Err(e) = spawned {
        state.running().remove(&name);
        return Err(err(Error::Io(e)));
    }
    Ok(())
}

fn record(state: &AppState, name: &str, f: &Finished) {
    let mut h = load_history(state, name);
    let rec = RunRecord {
        op: f.op,
        class: f.class.clone(),
        finished_at: f.finished_at,
        added: f.added,
        failed: f.failed,
    };
    if let Some(a) = f
        .phases
        .iter()
        .find(|ph| ph.phase == "archive" && (ph.class == "ok" || ph.class == "partial"))
    {
        h.last_archive = Some(RunRecord {
            op: Op::Archive,
            class: a.class.clone(),
            finished_at: f.finished_at,
            added: a.added,
            failed: a.failed,
        });
    }
    h.last_run = Some(rec);
    save_history(state, name, &h);
}

fn run_archive_phase(
    state: &AppState,
    p: &Profile,
    store: &mut FsSqliteStore,
    tracker: &mut Tracker,
    cancel: &Arc<AtomicBool>,
    sink: &dyn Sink,
) -> Result<RunSummary, Error> {
    tracker.phase("connect", Some(format!("Conectando a {}…", p.source.host)));
    sink.progress(&tracker.snapshot());
    let mut source = state.backend.source(p, state.secrets.as_ref())?;
    tracker.phase("archive", Some("Listando pastas…".into()));
    sink.progress(&tracker.snapshot());
    let flag = cancel.clone();
    with_events(tracker, sink, cancel, move |tx| {
        service::archive_with(p, source.as_mut(), store, Some(flag), &tx)
    })
}

fn run_export_phase(
    state: &AppState,
    p: &Profile,
    store: &mut FsSqliteStore,
    tracker: &mut Tracker,
    cancel: &Arc<AtomicBool>,
    sink: &dyn Sink,
) -> Result<RunSummary, Error> {
    let msg = match &p.target {
        Some(TargetSpec::Imap(e)) => format!("Conectando a {}…", e.host),
        _ => "Preparando a pasta de destino…".into(),
    };
    tracker.phase("connect", Some(msg));
    sink.progress(&tracker.snapshot());
    service::check_target(p, state.secrets.as_ref(), None)?;
    let target = state.backend.target(p, state.secrets.as_ref())?;
    tracker.phase("export", None);
    sink.progress(&tracker.snapshot());
    let flag = cancel.clone();
    with_events(tracker, sink, cancel, move |tx| {
        service::export_with(p, store, target, Some(flag), &tx)
    })
}

fn execute(
    state: &AppState,
    p: &Profile,
    op: Op,
    cancel: &Arc<AtomicBool>,
    sink: &dyn Sink,
) -> Finished {
    let mut tracker = Tracker::new(&p.name, op, "connect");
    let mut phases = vec![];
    let mut fatal: Option<String> = None;

    let opened = match op {
        Op::Archive | Op::Migrate => service::open_for_archive(p),
        Op::Export => service::open_existing(p, true),
    };
    match opened {
        Err(e) => {
            fatal = Some(humanize(&e));
            phases.push(PhaseResult {
                phase: if op == Op::Export {
                    "export"
                } else {
                    "archive"
                }
                .into(),
                class: "error".into(),
                added: 0,
                duplicates: 0,
                failed: 0,
                error: fatal.clone(),
            });
        }
        Ok(mut store) => {
            if matches!(op, Op::Archive | Op::Migrate) {
                let r = run_archive_phase(state, p, &mut store, &mut tracker, cancel, sink);
                let ph = phase_result("archive", &r, cancel);
                if r.is_err() {
                    fatal = ph.error.clone();
                }
                phases.push(ph);
            }
            let go_on = fatal.is_none() && !cancel.load(Ordering::SeqCst);
            if matches!(op, Op::Export | Op::Migrate) && go_on {
                let r = run_export_phase(state, p, &mut store, &mut tracker, cancel, sink);
                let ph = phase_result("export", &r, cancel);
                if r.is_err() {
                    fatal = ph.error.clone();
                }
                phases.push(ph);
            }
        }
    }

    // Cancelled only if it actually cut a phase short (or kept a migration from exporting).
    let cancelled = phases.iter().any(|p| p.class == "cancelled")
        || (op == Op::Migrate && fatal.is_none() && phases.len() < 2);
    let failed: usize = tracker.failures_total;
    let class = if cancelled {
        "cancelled"
    } else if fatal.is_some() {
        "error"
    } else if failed > 0 || phases.iter().any(|p| p.class == "partial") {
        "partial"
    } else {
        "ok"
    };
    Finished {
        profile: p.name.clone(),
        op,
        class: class.into(),
        added: phases.iter().map(|p| p.added).sum(),
        duplicates: phases.iter().map(|p| p.duplicates).sum(),
        failed,
        failures: tracker.failures,
        failures_total: tracker.failures_total,
        error: if cancelled {
            Some(
                "Cancelado pelo usuário. O que já foi gravado continua no backup; a próxima \
                  execução continua de onde parou."
                    .into(),
            )
        } else {
            fatal
        },
        phases,
        finished_at: now(),
    }
}

/// Asks the running operation of `name` to stop at its next server call.
pub fn cancel(state: &AppState, name: &str) -> Result<(), String> {
    match state.running().get(name) {
        Some(r) => {
            r.cancel.store(true, Ordering::SeqCst);
            Ok(())
        }
        None => Err(cfg("Não há operação em andamento para esta conta.")),
    }
}

/// Validates a folder the window asks to open: an existing directory, given as a full path.
pub fn folder_to_open(path: &str) -> Result<PathBuf, String> {
    let p = Path::new(path.trim());
    if path.trim().is_empty() || !p.is_absolute() {
        return Err(cfg("Caminho inválido."));
    }
    if !p.is_dir() {
        return Err(cfg(format!(
            "A pasta \"{}\" não existe (ainda). Ela é criada no primeiro arquivamento.",
            p.display()
        )));
    }
    Ok(p.to_path_buf())
}

#[cfg(test)]
mod tests;
