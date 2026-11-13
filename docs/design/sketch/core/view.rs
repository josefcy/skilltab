//! The only types that cross the IPC boundary. tauri-specta generates `bindings.ts` from these,
//! so the TS types are derived from them and never kept in sync by hand. Model types are not
//! Serialize. Building a view from the model is a pure function in this module.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    pub revision: u64,
    /// false until the first ingest finishes. The UI shows the usage columns as pending until then.
    pub usage_ready: bool,
    pub rows: Vec<SkillRow>,
    /// Shelved (disabled or removed) mounts that can be restored.
    pub shelf: Vec<ShelfRow>,
    pub broken_links: Vec<BrokenLinkRow>,
    /// Usage keys that match no installed body. Built-in slash commands are filtered out.
    pub ghosts: Vec<GhostRow>,
    pub facets: Facets,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillRow {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub mounts: Vec<MountSummary>,
    pub origin: OriginView,
    pub jstack: Option<JstackRelationView>,
    pub bundle: Option<BundleSummary>,
    pub usage: UsageSummary,
    pub flags: Vec<Flag>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MountSummary {
    pub id: String,
    pub harness: String,
    pub scope: ScopeView,
    pub owner: OwnerView,
    pub state: MountStateView,
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ScopeView { User, Project { root: String }, Bundle { bundle: String } }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum OwnerView { JstackSync, SkillsCli, PluginManager, ClaudeAiSync, HarnessBuiltin, Skilltab, Unmanaged }

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum MountStateView { Active, Shadowed { by: String }, BundleDisabled }

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum OriginView {
    JstackRepo { skill: String },
    SkillsCli { source: String, git_ref: Option<String> },
    Plugin { bundle: String },
    ClaudeAiSync,
    HarnessBuiltin { harness: String },
    Unknown,
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum JstackRelationView {
    Owned { skill: String, drift: DriftView },
    CopyOf { skill: String, drift: DriftView },
    Declared { source: String },
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DriftView { InSync, UncommittedEdits { files: Vec<String> }, Behind { commits: u32 }, Diverged }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct BundleSummary { pub id: String, pub name: String, pub marketplace: String, pub version: Option<String>, pub enabled: bool }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub total: u32,
    pub last30d: u32,
    pub last_used: Option<String>,
    /// null = never used. Computed in Rust so "unused" means the same thing on every screen.
    pub days_since_last_use: Option<u32>,
    pub by_harness: Vec<HarnessCount>,
}

#[derive(Serialize, Type)]
pub struct HarnessCount { pub harness: String, pub count: u32 }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum Flag { Shadowed, ManifestIssue, DuplicateContent, NameCollision }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct Facets { pub harnesses: Vec<FacetValue>, pub scopes: Vec<FacetValue>, pub origins: Vec<FacetValue> }

#[derive(Serialize, Type)]
pub struct FacetValue { pub id: String, pub label: String, pub count: u32, pub present: bool }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ShelfRow { pub id: String, pub harness: String, pub entry: String, pub reason: String, pub at: String }

#[derive(Serialize, Type)]
pub struct BrokenLinkRow { pub id: String, pub harness: String, pub link: String, pub target: String }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GhostRow { pub harness: String, pub key: String, pub total: u32, pub last_used: String }

// ---------------------------------------------------------------- detail

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SkillDetail {
    pub row: SkillRow,
    pub dir: String,
    pub skill_md: String,
    pub files: Vec<String>,
    pub mounts: Vec<MountDetail>,
    pub series: Vec<UsagePointView>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct MountDetail { pub summary: MountSummary, pub entry: String, pub chain: Vec<String>, pub keys: Vec<String> }

#[derive(Serialize, Type)]
pub struct UsagePointView { pub day: String, pub harness: String, pub kind: String, pub count: u32 }

// ---------------------------------------------------------------- actions

/// Deserialized from the UI and parsed into manage::ManageAction against the current snapshot.
#[derive(Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ActionRequest {
    Remove { target: RemoveTargetRequest },
    Disable { mount: String },
    Restore { shelf: String },
    Bundle { bundle: String, op: BundleOpRequest },
    Jstack { op: JstackOpRequest },
}

#[derive(Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RemoveTargetRequest { Mount { mount: String }, Everywhere { skill: String }, BrokenLink { id: String } }

#[derive(Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum BundleOpRequest { Enable, Disable, Uninstall, Update }

#[derive(Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum JstackOpRequest {
    Sync,
    PullAndSync,
    InstallDeclared { source: String, skills: Vec<String> },
    UpdateDeclared { source: String, skills: Vec<String> },
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PlanView {
    pub id: String,
    pub steps: Vec<StepView>,
    pub effects: Vec<EffectView>,
    pub blockers: Vec<BlockerView>,
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StepView {
    RemoveLink { path: String },
    Recycle { path: String },
    Stash { path: String },
    Relink { entry: String, target: String },
    Unstash { to: String },
    Run { command: String, why: String },
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EffectView {
    HarnessLosesSkill { harness: String },
    AlsoBreaks { harness: String, entry: String, via: String },
    BodyRecycled { path: String },
    UsageBecomesGhost,
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BlockerView {
    RecreatedBy { by: OwnerView, remedy: String },
    PartOfBundle { bundle: String },
    HarnessBuiltin,
    InsideJstackRepo,
    SharedBody { dependents: Vec<String> },
    Unsupported { why: String },
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum ApplyView {
    Stale { reasons: Vec<String> },
    Applied { steps: Vec<StepResultView>, output: Option<String>, revision: u64 },
    Refused,
}

#[derive(Serialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum StepResultView { Done, Skipped, Failed { message: String } }

// ---------------------------------------------------------------- jstack, refresh, settings

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JstackView {
    pub root: String,
    pub remote: Option<String>,
    pub head: String,
    pub behind: Option<u32>,
    pub ahead: u32,
    pub dirty: bool,
    pub fetched_at: Option<String>,
    /// Repo skills joined with their installed rows (skill ids) and their drift.
    pub own: Vec<JstackSkillView>,
    pub declared: Vec<JstackDeclaredView>,
}

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JstackSkillView { pub name: String, pub installed_as: Vec<String>, pub drift: Option<DriftView> }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct JstackDeclaredView { pub source: String, pub skill: String, pub installed_as: Option<String> }

#[derive(Serialize, Type, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IngestProgressView { pub files_done: u32, pub files_total: u32, pub bytes_done: u64, pub bytes_total: u64 }

#[derive(Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct RefreshReport { pub revision: u64, pub events_added: u64, pub bytes_read: u64, pub issues: Vec<String> }

#[derive(Deserialize, Type)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum RevealTarget { Mount { id: String }, Skill { id: String } }

#[derive(Serialize, Deserialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView { pub jstack_root: Option<String>, pub extra_projects: Vec<String> }
