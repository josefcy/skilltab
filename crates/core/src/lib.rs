//! skilltab-core. A plain Rust library with no Tauri dependency. The app crate is a thin shell over `Skilltab`.
//!
//! `Skilltab` is the facade. Callers see `view` types only, so the model can change without touching the shell.

// These modules carry hooks for the usage and manage units (parsers, bundle commands, plan re-checks) that nothing calls yet.
mod fsutil;
#[allow(dead_code, unused_variables)]
mod harness;
mod jstack;
#[allow(dead_code)]
mod model;
mod projects;
#[allow(dead_code)]
mod provenance;
#[allow(dead_code, unused_imports, unused_variables)]
mod scan;
mod usage;
pub mod view;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use view::{CoreError, Inventory, RefreshReport, SkillDetail, Tallies};

pub use projects::discover_projects;

/// Read-only model access for the `dump` example. The app crate never needs these.
pub use model::{
    Bundle, BundleId, ContentHash, InstallScope, InvocationKey, Manifest, ManifestIssue, Mount, MountId,
    MountState, Origin, Ownership, Scope, ScanIssue, SkillBody, SkillId, Snapshot,
};
pub use model::{HarnessId, HarnessInfo, LinkPath, ProjectRoot, RealDir, Tally, TrackingSince};
pub use harness::InvocationKind;
pub use jstack::{fetch as jstack_fetch, report as jstack_report, Absence, DeclaredMissing, DeclaredSource, Drift, JstackAction, JstackCommand, JstackError, JstackReport, JstackRelation, NotInstalled, RepoSkill, RepoState, Upstream};
pub use usage::{IngestProgress, IngestReport, UsageError, UsageIndex, UsagePoint};

pub struct Env {
    pub home: PathBuf,
    /// Project roots to check beyond the ones found in transcripts.
    pub projects: Vec<PathBuf>,
    /// The jstack clone. None means no skill is attributed to jstack.
    pub jstack_root: Option<PathBuf>,
}

pub struct Skilltab {
    env: Env,
    data_dir: PathBuf,
    /// Replaced whole by `refresh`. Readers clone the Arcs and never wait on a scan.
    current: RwLock<Current>,
}

#[derive(Clone)]
struct Current {
    snapshot: Arc<Snapshot>,
    usage: Arc<Usage>,
    scan_ms: u32,
}

#[derive(Default)]
struct Usage {
    tallies: Tallies,
    tracking_since: model::TrackingSince,
}

impl Skilltab {
    /// Builds the first snapshot, so the window can show the inventory at once.
    pub fn open(env: Env, data_dir: PathBuf) -> Result<Skilltab, CoreError> {
        let current = build(&env, &data_dir, 1);
        Ok(Skilltab { env, data_dir, current: RwLock::new(current) })
    }

    /// Rescans the disk and bumps the revision.
    pub fn refresh(&self) -> RefreshReport {
        let next = build(&self.env, &self.data_dir, self.read().snapshot.revision + 1);
        let report = RefreshReport {
            revision: next.snapshot.revision as u32,
            rows: next.snapshot.bodies.len() as u32,
            scan_ms: next.scan_ms,
        };
        *self.current.write().unwrap_or_else(|e| e.into_inner()) = next;
        report
    }

    /// Every row, bundle, problem and facet. Filtering is client-side.
    pub fn inventory(&self) -> Inventory {
        let c = self.read();
        view::inventory(&c.snapshot, &c.usage.tallies, &c.usage.tracking_since, c.scan_ms)
    }

    pub fn skill(&self, id: &str) -> Result<SkillDetail, CoreError> {
        let c = self.read();
        view::detail(&c.snapshot, id, &c.usage.tallies, &c.usage.tracking_since)
    }

    /// Opens Explorer on one hop of one mount's chain (0 is the entry, the last is the real folder).
    /// Takes ids the core issued, never a path.
    pub fn reveal(&self, mount_id: &str, hop: u32) -> Result<(), CoreError> {
        let snap = self.read().snapshot;
        let mount = snap
            .mounts
            .values()
            .find(|m| m.id.as_str() == mount_id)
            .ok_or_else(|| CoreError::UnknownId { id: mount_id.to_owned() })?;
        let path = view::chain_paths(&snap, mount)
            .into_iter()
            .nth(hop as usize)
            .ok_or_else(|| CoreError::UnknownId { id: format!("{mount_id}#{hop}") })?;
        reveal_in_explorer(&path).map_err(|e| CoreError::Io { message: e.to_string() })
    }

    /// The raw snapshot of the last refresh, for the `dump` example.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.read().snapshot
    }

    fn read(&self) -> Current {
        self.current.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn build(env: &Env, data_dir: &Path, revision: u64) -> Current {
    let started = Instant::now();
    let snapshot = scan_disk(env, revision);
    let usage = load_usage(env, data_dir);
    Current { snapshot: Arc::new(snapshot), usage: Arc::new(usage), scan_ms: started.elapsed().as_millis() as u32 }
}

/// One scan of every harness root, with no usage. Shared by the facade and by tests that only need a snapshot.
pub(crate) fn scan_disk(env: &Env, revision: u64) -> Snapshot {
    let projects = scan::project_roots(&project_candidates(env));
    let provenance = provenance::Provenance::load(env, env.jstack_root.as_deref());
    scan::scan(env, harness::registry(), &projects, &provenance, Vec::new(), revision)
}

/// Projects named in settings plus the ones transcripts mention. The usage index will supply the second part.
fn project_candidates(env: &Env) -> Vec<PathBuf> {
    let mut all = env.projects.clone();
    all.extend(discover_projects(&env.home));
    all
}

/// Brings the usage index up to date with the transcripts, then reads its tallies.
/// A failure leaves usage empty, so the columns read "no data yet" rather than a wrong zero.
fn load_usage(env: &Env, data_dir: &Path) -> Usage {
    let read = || -> usage::Result<Usage> {
        let mut index = UsageIndex::open(&data_dir.join("usage.db"))?;
        index.ingest_all(env, &mut |_| {})?;
        Ok(Usage { tallies: index.totals(chrono::Utc::now())?, tracking_since: index.tracking_since()? })
    };
    read().unwrap_or_else(|e| {
        eprintln!("skilltab: usage index unavailable: {e}");
        Usage::default()
    })
}

#[cfg(windows)]
fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // Explorer parses /select, itself and wants the quotes literal. Explorer also exits 1 on success.
    std::process::Command::new("explorer.exe").raw_arg(format!("/select,\"{}\"", view::path_str(path))).spawn().map(|_| ())
}

#[cfg(not(windows))]
fn reveal_in_explorer(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(std::io::ErrorKind::Unsupported, "reveal is only implemented on Windows"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{HarnessId, InvocationKey, Tally};
    use std::fs;

    fn skill(home: &Path, rel: &str, name: &str) {
        let dir = home.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: does {name}\n---\n# {name}\nbody\n")).unwrap();
    }

    fn open(home: &Path) -> Skilltab {
        let env = Env { home: home.to_path_buf(), projects: Vec::new(), jstack_root: None };
        Skilltab::open(env, home.join("data")).unwrap()
    }

    #[test]
    fn usage_reads_no_data_until_a_harness_is_tracked_and_unused_only_after() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        let app = open(tmp.path());
        let snap = app.snapshot();

        let none = view::inventory(&snap, &Tallies::new(), &Default::default(), 0);
        assert_eq!(none.rows.len(), 1);
        assert!(!none.rows[0].usage.tracked);
        assert!(!none.rows[0].usage.unused);

        let mut tracking = model::TrackingSince::new();
        tracking.insert(HarnessId::CLAUDE_CODE, chrono::Utc::now());
        let tracked = view::inventory(&snap, &Tallies::new(), &tracking, 0);
        assert!(tracked.rows[0].usage.tracked);
        assert!(tracked.rows[0].usage.unused);
        assert_eq!(tracked.tracking_since.len(), 1);

        let mut tallies = Tallies::new();
        let used = Tally { total: 4, last_30d: 2, last_used: Some(chrono::Utc::now()), by_kind: [0; 4] };
        tallies.insert((HarnessId::CLAUDE_CODE, InvocationKey::Name("tdd".into())), used);
        let used = view::inventory(&snap, &tallies, &tracking, 0);
        let u = &used.rows[0].usage;
        assert_eq!((u.total, u.last30d, u.unused), (4, 2, false));
        assert!(u.days_since_last_use.is_some());
    }

    #[test]
    fn two_bodies_with_one_name_are_flagged_and_listed_as_a_conflict() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/graphify", "graphify");
        skill(tmp.path(), ".codex/skills/graphify", "graphify");
        skill(tmp.path(), ".codex/skills/solo", "solo");
        let inv = open(tmp.path()).inventory();

        let flagged: Vec<&str> =
            inv.rows.iter().filter(|r| r.flags.contains(&view::Flag::NameConflict)).map(|r| r.name.as_str()).collect();
        assert_eq!(flagged, ["graphify", "graphify"]);
        assert_eq!(inv.problems.name_conflicts.len(), 1);
        assert_eq!(inv.problems.name_conflicts[0].members.len(), 2);
        assert!(inv.problems.name_conflicts[0].identical_content);
        let facet = |id: &str| inv.facets.harnesses.iter().find(|h| h.id == id).map(|h| h.count);
        assert_eq!((facet("claude-code"), facet("codex")), (Some(1), Some(2)));
    }

    #[test]
    fn detail_splits_frontmatter_and_ends_the_chain_at_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        let app = open(tmp.path());
        let id = app.inventory().rows[0].id.clone();

        let d = app.skill(&id).unwrap();
        assert_eq!(d.frontmatter.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["name", "description"]);
        assert!(d.markdown.starts_with("# tdd"));
        assert_eq!(d.mounts.len(), 1);
        let chain = &d.mounts[0].chain;
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0].kind, view::HopKind::Folder);
        assert!(matches!(app.skill("nope"), Err(CoreError::UnknownId { .. })));
        assert!(matches!(app.reveal(&d.mounts[0].summary.id, 5), Err(CoreError::UnknownId { .. })));
    }

    #[test]
    fn refresh_bumps_the_revision() {
        let tmp = tempfile::tempdir().unwrap();
        let app = open(tmp.path());
        let before = app.inventory().revision;
        assert_eq!(app.refresh().revision, before + 1);
        assert_eq!(app.inventory().revision, before + 1);
    }
}
