//! Each command is one line into `Skilltab`. No logic lives here.
//! `collect_commands!` drives both the IPC handler and `src/bindings.ts`, so the two cannot disagree.

use serde::{Deserialize, Serialize};
use skilltab_core::view::{ActionRequest, AliasSuggestionView, ApplyReport, CoreError, Inventory, JstackView, PlanView, RefreshReport, SkillDetail};
use skilltab_core::Skilltab;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager, State};
use tauri_specta::{collect_commands, collect_events, Builder, ErrorHandlingMode, Event};

/// Emitted after every refresh. The UI re-fetches the inventory when `revision` changes.
#[derive(Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct InventoryChanged {
    pub revision: u32,
}

/// Held for the duration of a refresh, so a burst of window-focus events costs one scan.
#[derive(Default)]
pub struct RefreshGuard(AtomicBool);

#[tauri::command(async)]
#[specta::specta]
fn get_inventory(app: State<'_, Skilltab>) -> Inventory {
    app.inventory()
}

#[tauri::command(async)]
#[specta::specta]
fn get_skill(app: State<'_, Skilltab>, id: String) -> Result<SkillDetail, CoreError> {
    app.skill(&id)
}

#[tauri::command(async)]
#[specta::specta]
fn reveal(app: State<'_, Skilltab>, mount_id: String, hop: u32) -> Result<(), CoreError> {
    app.reveal(&mount_id, hop)
}

#[tauri::command(async)]
#[specta::specta]
fn refresh(handle: AppHandle) -> Option<RefreshReport> {
    refresh_and_notify(&handle)
}

#[tauri::command(async)]
#[specta::specta]
fn plan_action(app: State<'_, Skilltab>, action: ActionRequest) -> Result<PlanView, CoreError> {
    app.plan(&action)
}

/// Apply ends with a rescan, so the UI hears about the new revision like after any refresh.
#[tauri::command(async)]
#[specta::specta]
fn apply_plan(handle: AppHandle, plan_id: String) -> Result<ApplyReport, CoreError> {
    let report = handle.state::<Skilltab>().apply(&plan_id)?;
    let _ = InventoryChanged { revision: report.revision }.emit(&handle);
    Ok(report)
}

/// None when the machine has no jstack clone. Reads the last refresh and runs no git.
#[tauri::command(async)]
#[specta::specta]
fn get_jstack(app: State<'_, Skilltab>) -> Option<JstackView> {
    app.jstack()
}

/// The one jstack call that reaches the network. It rescans, so the UI hears about the new revision.
#[tauri::command(async)]
#[specta::specta]
fn jstack_fetch(handle: AppHandle) -> Result<RefreshReport, CoreError> {
    let report = handle.state::<Skilltab>().jstack_fetch()?;
    let _ = InventoryChanged { revision: report.revision }.emit(&handle);
    Ok(report)
}

#[tauri::command(async)]
#[specta::specta]
fn alias_suggestions(app: State<'_, Skilltab>) -> Vec<AliasSuggestionView> {
    app.alias_suggestions()
}

#[tauri::command(async)]
#[specta::specta]
fn confirm_alias(handle: AppHandle, harness: String, from: String, to: String) -> Result<RefreshReport, CoreError> {
    let report = handle.state::<Skilltab>().confirm_alias(&harness, &from, &to)?;
    let _ = InventoryChanged { revision: report.revision }.emit(&handle);
    Ok(report)
}

#[tauri::command(async)]
#[specta::specta]
fn remove_alias(handle: AppHandle, harness: String, from: String) -> Result<RefreshReport, CoreError> {
    let report = handle.state::<Skilltab>().remove_alias(&harness, &from)?;
    let _ = InventoryChanged { revision: report.revision }.emit(&handle);
    Ok(report)
}

/// None when another refresh is already running.
pub fn refresh_and_notify(handle: &AppHandle) -> Option<RefreshReport> {
    let guard = handle.state::<RefreshGuard>();
    if guard.0.swap(true, Ordering::Acquire) {
        return None;
    }
    let report = handle.state::<Skilltab>().refresh();
    guard.0.store(false, Ordering::Release);
    let _ = InventoryChanged { revision: report.revision }.emit(handle);
    Some(report)
}

pub fn builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .error_handling(ErrorHandlingMode::Result)
        .disable_serde_phases()
        .commands(collect_commands![
            get_inventory,
            get_skill,
            reveal,
            refresh,
            plan_action,
            apply_plan,
            get_jstack,
            jstack_fetch,
            alias_suggestions,
            confirm_alias,
            remove_alias
        ])
        .events(collect_events![InventoryChanged])
}
