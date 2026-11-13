//! skilltab-core. A plain Rust library with no Tauri dependency. The Tauri crate is a thin shell over `Skilltab`.
//!
//! Public surface: `Skilltab`, `Env`, `Settings`, and the `view` types. Everything else is
//! `pub(crate)`, so the shell (or a future CLI) can only go through the facade.

mod harness;
mod jstack;
mod manage;
mod model;
mod provenance;
mod scan;
mod usage;
pub mod view;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use view::*;

pub struct Env {
    pub home: PathBuf,
    /// %LOCALAPPDATA%\skilltab
    pub data_dir: PathBuf,
}

pub struct Settings {
    /// Default ~/jstack. None = jstack features are hidden.
    pub jstack_root: Option<PathBuf>,
    pub extra_projects: Vec<PathBuf>,
}

/// Owns all runtime state. Each piece has one writer:
///   snapshot: replaced whole by `refresh` (readers clone the Arc and never wait on a scan)
///   usage:    written only by `refresh` (ingest). Reads use a second WAL connection inside UsageIndex.
///   shelf:    written only by `apply`
///   plans:    written by `plan`, consumed by `apply`; only plans for the current revision are kept
pub struct Skilltab {
    env: Env,
    settings: Mutex<Settings>,
    snapshot: arc_swap::ArcSwap<model::Snapshot>,
    usage: Mutex<usage::UsageIndex>,
    jstack: arc_swap::ArcSwapOption<jstack::JstackRepo>,
    shelf: Mutex<manage::Shelf>,
    plans: Mutex<std::collections::HashMap<manage::PlanId, manage::Plan>>,
    /// Serializes refresh and apply. Two refreshes at once would ingest the same bytes twice (harmless,
    /// dedup absorbs it) but would waste the 442 MB first pass. An apply racing a scan would plan on stale state.
    writer: tokio::sync::Mutex<()>,
}

impl Skilltab {
    /// Opens the stores and builds the first snapshot *without* ingesting, so the window shows the
    /// inventory at once. Usage columns read "indexing…" until the first `refresh` finishes.
    pub fn open(env: Env, settings: Settings) -> Result<Skilltab, CoreError> { unimplemented!() }

    /// Ingest new transcript bytes, reload jstack (no fetch), rescan, bump the revision.
    pub async fn refresh(&self, progress: impl FnMut(IngestProgressView) + Send) -> Result<RefreshReport, CoreError> { unimplemented!() }

    /// All rows and facets. Filtering is client-side (about 300 rows), so this is the only list query.
    pub fn inventory(&self) -> Inventory { unimplemented!() }

    pub fn skill(&self, id: &str) -> Result<SkillDetail, CoreError> { unimplemented!() }

    /// `fetch: true` runs `git fetch` first. That needs the network; the UI calls it on demand.
    pub async fn jstack(&self, fetch: bool) -> Result<JstackView, CoreError> { unimplemented!() }

    /// Parses the request's ids against the current snapshot (an unknown id → CoreError::UnknownId),
    /// builds the plan and keeps it.
    pub fn plan(&self, request: ActionRequest) -> Result<PlanView, CoreError> { unimplemented!() }

    /// Refuses a plan from an older revision (Stale). On success it runs `refresh` before it returns.
    pub async fn apply(&self, plan_id: &str) -> Result<ApplyView, CoreError> { unimplemented!() }

    /// Opens Explorer at a mount entry or a body dir. Takes an id, never a path.
    pub fn reveal(&self, target: RevealTarget) -> Result<(), CoreError> { unimplemented!() }

    pub fn settings(&self) -> SettingsView { unimplemented!() }
    pub fn set_settings(&self, s: SettingsView) -> Result<(), CoreError> { unimplemented!() }
}

#[derive(Debug, serde::Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum CoreError {
    UnknownId { id: String },
    StalePlan,
    Io { message: String },
    Index { message: String },
    Jstack { message: String },
}
