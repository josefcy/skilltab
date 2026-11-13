//! skilltab-core. A plain Rust library with no Tauri dependency.
//!
//! Unit 1 builds a `Snapshot` of every installed skill. The modules stay private so the shell
//! can only go through what is re-exported here.

// Fields, stubs and helpers here are consumed by the usage, manage and view units.
#![allow(dead_code, unused_variables, unused_imports)]

mod fsutil;
mod harness;
mod jstack;
mod model;
mod provenance;
mod scan;
mod usage;

use std::path::PathBuf;

/// Read-only inspection surface for the example and tests. The `view` types replace it once they exist.
pub use model::{
    Bundle, BundleId, ContentHash, InstallScope, InvocationKey, Manifest, ManifestIssue, Mount, MountId,
    MountState, Origin, Ownership, Scope, ScanIssue, SkillBody, SkillId, Snapshot,
};
pub use model::{HarnessId, HarnessInfo, LinkPath, ProjectRoot, RealDir, Tally, TrackingSince};
pub use harness::InvocationKind;
pub use usage::{IngestProgress, IngestReport, UsageError, UsageIndex, UsagePoint};

pub struct Env {
    pub home: PathBuf,
    /// Project roots to check for `.claude/skills`, `.agents/skills` and `.codex/skills`.
    pub projects: Vec<PathBuf>,
    /// The jstack clone. None means no skill is attributed to jstack.
    pub jstack_root: Option<PathBuf>,
}

/// Scans the machine described by `env`. Read-only. Problems found on disk become `Snapshot::issues`.
pub fn snapshot(env: &Env) -> Snapshot {
    let projects = scan::project_roots(&env.projects);
    let provenance = provenance::Provenance::load(env, env.jstack_root.as_deref());
    scan::scan(env, harness::registry(), &projects, &provenance, Vec::new(), 1)
}
