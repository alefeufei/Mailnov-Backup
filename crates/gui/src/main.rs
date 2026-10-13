//! MailBackup desktop app: a Tauri v2 window over `mailbackup-core`.
//!
//! The commands below are thin: each one forwards to a plain function in [`logic`] (tested
//! without a window). Long operations run on worker threads and report through the `progress`
//! and `finished` events. Passwords only travel from the window into Rust and then into the
//! Windows Credential Manager; they are never logged, stored in files or sent back.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod logic;

use logic::{
    AppState, Detected, EndpointInput, Finished, ImapBackend, Op, ProfileInput, ProfileSummary,
    Progress, Sink, TargetInput, VerifyResult,
};
use mailbackup_core::service::StatusReport;
use mailbackup_core::KeyringSecrets;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

type AppStateRef<'a> = State<'a, Arc<AppState>>;

/// Runs blocking work (disk, network, credential store) off the async runtime's threads.
async fn blocking<T, F>(state: &AppStateRef<'_>, f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce(&AppState) -> Result<T, String> + Send + 'static,
{
    let st = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || f(&st))
        .await
        .map_err(|e| format!("Erro interno: {e}"))?
}

struct WindowSink(AppHandle);

impl Sink for WindowSink {
    fn progress(&self, p: &Progress) {
        let _ = self.0.emit("progress", p);
    }
    fn finished(&self, f: &Finished) {
        let _ = self.0.emit("finished", f);
    }
}

#[tauri::command]
async fn list_profiles(state: AppStateRef<'_>) -> Result<Vec<ProfileSummary>, String> {
    blocking(&state, logic::list_profiles).await
}

#[tauri::command]
async fn detect_server(state: AppStateRef<'_>, email: String) -> Result<Detected, String> {
    blocking(&state, move |s| logic::detect_server(s, &email)).await
}

#[tauri::command]
async fn test_connection(
    state: AppStateRef<'_>,
    endpoint: EndpointInput,
    password: String,
) -> Result<Vec<String>, String> {
    blocking(&state, move |s| {
        logic::test_connection(s, &endpoint, password)
    })
    .await
}

#[tauri::command]
async fn save_profile(state: AppStateRef<'_>, input: ProfileInput) -> Result<(), String> {
    blocking(&state, move |s| logic::save_profile(s, input)).await
}

#[tauri::command]
async fn set_target(
    state: AppStateRef<'_>,
    name: String,
    target: TargetInput,
) -> Result<(), String> {
    blocking(&state, move |s| logic::set_target(s, &name, target)).await
}

#[tauri::command]
async fn delete_profile(state: AppStateRef<'_>, name: String) -> Result<(), String> {
    blocking(&state, move |s| logic::delete_profile(s, &name)).await
}

fn start(app: AppHandle, state: &AppStateRef<'_>, name: &str, op: Op) -> Result<(), String> {
    logic::start(state.inner().clone(), name, op, Arc::new(WindowSink(app)))
}

#[tauri::command]
async fn start_archive(app: AppHandle, state: AppStateRef<'_>, name: String) -> Result<(), String> {
    start(app, &state, &name, Op::Archive)
}

#[tauri::command]
async fn start_export(app: AppHandle, state: AppStateRef<'_>, name: String) -> Result<(), String> {
    start(app, &state, &name, Op::Export)
}

#[tauri::command]
async fn start_migrate(app: AppHandle, state: AppStateRef<'_>, name: String) -> Result<(), String> {
    start(app, &state, &name, Op::Migrate)
}

#[tauri::command]
fn cancel(state: AppStateRef<'_>, name: String) -> Result<(), String> {
    logic::cancel(&state, &name)
}

#[tauri::command]
async fn run_verify(state: AppStateRef<'_>, name: String) -> Result<VerifyResult, String> {
    blocking(&state, move |s| logic::run_verify(s, &name)).await
}

#[tauri::command]
async fn run_reindex(state: AppStateRef<'_>, name: String) -> Result<usize, String> {
    blocking(&state, move |s| logic::run_reindex(s, &name)).await
}

#[tauri::command]
async fn get_status(state: AppStateRef<'_>, name: String) -> Result<StatusReport, String> {
    blocking(&state, move |s| logic::get_status(s, &name)).await
}

/// Opens an existing directory in Explorer (never a file, so nothing can be executed).
#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    let dir = logic::folder_to_open(&path)?;
    #[cfg(windows)]
    let program = "explorer.exe";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    std::process::Command::new(program)
        .arg(&dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Não foi possível abrir a pasta: {e}"))
}

/// Native folder picker; `None` when the user closes it.
#[tauri::command]
async fn pick_folder(
    window: tauri::WebviewWindow,
    title: String,
    start: Option<String>,
) -> Result<Option<String>, String> {
    let mut dialog = window.dialog().file().set_title(title).set_parent(&window);
    if let Some(dir) = start
        .map(std::path::PathBuf::from)
        .and_then(|d| d.ancestors().find(|a| a.is_dir()).map(|a| a.to_path_buf()))
    {
        dialog = dialog.set_directory(dir);
    }
    let picked = tauri::async_runtime::spawn_blocking(move || dialog.blocking_pick_folder())
        .await
        .map_err(|e| format!("Erro interno: {e}"))?;
    Ok(picked
        .and_then(|p| p.into_path().ok())
        .map(|p| p.display().to_string()))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let dir = logic::default_config_dir()
                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
            let state = AppState::new(dir, Box::new(KeyringSecrets), Box::new(ImapBackend));
            app.manage(Arc::new(state));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_profiles,
            detect_server,
            test_connection,
            save_profile,
            set_target,
            delete_profile,
            start_archive,
            start_export,
            start_migrate,
            cancel,
            run_verify,
            run_reindex,
            get_status,
            open_folder,
            pick_folder,
        ])
        .run(tauri::generate_context!())
        .expect("error while running MailBackup");
}
