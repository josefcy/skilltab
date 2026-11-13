//! skilltab-app: the Tauri 2 shell. Each command is one line into `Skilltab`. No logic lives here.
//! The capability file grants the webview only these commands plus `event:listen`. There is no fs and no shell plugin.

use skilltab_core::view::*;
use skilltab_core::{CoreError, Skilltab};
use tauri::{AppHandle, State};
use tauri_specta::Event;

#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct IngestProgress(pub IngestProgressView);

#[tauri::command] #[specta::specta]
fn get_inventory(app: State<'_, Skilltab>) -> Inventory { app.inventory() }

#[tauri::command] #[specta::specta]
fn get_skill(app: State<'_, Skilltab>, id: String) -> Result<SkillDetail, CoreError> { app.skill(&id) }

#[tauri::command] #[specta::specta]
async fn get_jstack(app: State<'_, Skilltab>, fetch: bool) -> Result<JstackView, CoreError> { app.jstack(fetch).await }

#[tauri::command] #[specta::specta]
async fn refresh(handle: AppHandle, app: State<'_, Skilltab>) -> Result<RefreshReport, CoreError> {
    app.refresh(move |p| { let _ = IngestProgress(p).emit(&handle); }).await
}

#[tauri::command] #[specta::specta]
fn plan_action(app: State<'_, Skilltab>, action: ActionRequest) -> Result<PlanView, CoreError> { app.plan(action) }

#[tauri::command] #[specta::specta]
async fn apply_plan(app: State<'_, Skilltab>, plan: String) -> Result<ApplyView, CoreError> { app.apply(&plan).await }

#[tauri::command] #[specta::specta]
fn reveal(app: State<'_, Skilltab>, target: RevealTarget) -> Result<(), CoreError> { app.reveal(target) }

#[tauri::command] #[specta::specta]
fn get_settings(app: State<'_, Skilltab>) -> SettingsView { app.settings() }

#[tauri::command] #[specta::specta]
fn set_settings(app: State<'_, Skilltab>, settings: SettingsView) -> Result<(), CoreError> { app.set_settings(settings) }

/// One list. `collect_commands!` drives both the IPC handler and `bindings.ts`, so the two cannot disagree.
pub fn builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::new()
        .commands(tauri_specta::collect_commands![
            get_inventory, get_skill, get_jstack, refresh, plan_action, apply_plan, reveal, get_settings, set_settings
        ])
        .events(tauri_specta::collect_events![IngestProgress])
    // debug builds: .export(Typescript::default(), "../ui/src/bindings.ts")
}

pub fn run() {
    // setup: Env from dirs::home_dir + %LOCALAPPDATA%\skilltab; Skilltab::open; app.manage(...)
    // then spawn refresh() once in the background, and again whenever the window gains focus (cheap: about 350 stats).
    unimplemented!()
}
