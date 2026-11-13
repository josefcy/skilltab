//! skilltab-core. A plain Rust library with no Tauri dependency. The app crate is a thin shell over `Skilltab`.
//!
//! `Skilltab` is the facade. Callers see `view` types only, so the model can change without touching the shell.

mod fsutil;
mod manage;
// Hooks for the usage and jstack units that nothing calls yet.
#[allow(dead_code, unused_variables)]
mod harness;
mod model;
mod projects;
#[allow(dead_code)]
mod provenance;
#[allow(unused_variables)]
mod scan;
mod usage;
pub mod view;

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use view::{ActionRequest, ApplyOutcomeKind, ApplyReport, CoreError, Inventory, PlanView, RefreshReport, SkillDetail, Tallies};

pub use projects::discover_projects;

/// Read-only model access for the `dump` example. The app crate never needs these.
pub use model::{
    Bundle, BundleId, ContentHash, InstallScope, InvocationKey, Manifest, ManifestIssue, Mount, MountId,
    MountState, Origin, Ownership, Scope, ScanIssue, SkillBody, SkillId, Snapshot,
};
pub use model::{HarnessId, HarnessInfo, LinkPath, ProjectRoot, RealDir, Tally, TrackingSince};
pub use harness::InvocationKind;
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
    /// Written only by `apply`. Held across a whole apply, so two applies never interleave their steps.
    shelf: Mutex<manage::Shelf>,
    /// Plans handed to the UI, by id. Only a plan from the current revision can be applied.
    plans: Mutex<HashMap<String, manage::Plan>>,
    next_plan: AtomicU64,
    os: Box<dyn manage::Os>,
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
        Skilltab::open_with(env, data_dir, Box::new(manage::SystemOs))
    }

    fn open_with(env: Env, data_dir: PathBuf, os: Box<dyn manage::Os>) -> Result<Skilltab, CoreError> {
        let shelf = manage::Shelf::open(&data_dir).map_err(|e| CoreError::Io { message: e.to_string() })?;
        let current = build(&env, &data_dir, shelf.entries(), 1);
        Ok(Skilltab {
            env,
            data_dir,
            current: RwLock::new(current),
            shelf: Mutex::new(shelf),
            plans: Mutex::new(HashMap::new()),
            next_plan: AtomicU64::new(1),
            os,
        })
    }

    /// Rescans the disk and bumps the revision.
    pub fn refresh(&self) -> RefreshReport {
        let shelved = lock(&self.shelf).entries().to_vec();
        let next = build(&self.env, &self.data_dir, &shelved, self.read().snapshot.revision + 1);
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

    /// Plans a change against the current snapshot and keeps the plan for `apply`. Nothing on disk changes.
    pub fn plan(&self, request: &ActionRequest) -> Result<PlanView, CoreError> {
        let snap = self.read().snapshot;
        let action = view::parse_action(&snap, request)?;
        let plan = manage::plan(&snap, &lock(&self.shelf), action);
        let id = format!("{}-{}", snap.revision, self.next_plan.fetch_add(1, Ordering::Relaxed));
        let out = view::plan_view(&snap, id.clone(), &plan);
        let mut plans = lock(&self.plans);
        plans.retain(|_, p| p.revision == snap.revision);
        plans.insert(id, plan);
        Ok(out)
    }

    /// Runs a kept plan, then rescans. A plan from an older revision is refused as stale, so the UI plans again.
    pub fn apply(&self, plan_id: &str) -> Result<ApplyReport, CoreError> {
        let plan = lock(&self.plans).remove(plan_id).ok_or_else(|| CoreError::UnknownId { id: plan_id.to_owned() })?;
        let revision = self.read().snapshot.revision;
        if plan.revision != revision {
            let stale = manage::ApplyOutcome::Stale { reasons: vec!["The inventory changed since this plan was made.".to_owned()] };
            return Ok(view::apply_report(&plan, stale, revision as u32));
        }
        let outcome = manage::apply(&plan, &mut lock(&self.shelf), self.os.as_ref());
        let mut report = view::apply_report(&plan, outcome, revision as u32);
        if matches!(report.outcome, ApplyOutcomeKind::Applied | ApplyOutcomeKind::Failed) {
            report.revision = self.refresh().revision;
        }
        Ok(report)
    }

    /// The raw snapshot of the last refresh, for the `dump` example.
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.read().snapshot
    }

    fn read(&self) -> Current {
        self.current.read().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn build(env: &Env, data_dir: &Path, shelved: &[model::ShelfEntry], revision: u64) -> Current {
    let started = Instant::now();
    let projects = scan::project_roots(&project_candidates(env));
    let provenance = provenance::Provenance::load(env, env.jstack_root.as_deref());
    let snapshot = scan::scan(env, harness::registry(), &projects, &provenance, shelved.to_vec(), revision);
    let usage = load_usage(env, data_dir);
    Current { snapshot: Arc::new(snapshot), usage: Arc::new(usage), scan_ms: started.elapsed().as_millis() as u32 }
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

    mod manage_flow {
        use super::super::*;
        use crate::manage::tests::{FakeOs, Home};
        use std::sync::Arc;
        use view::{ActionRequest, ApplyOutcomeKind, BlockerKind, StepResultView};

        /// Shares the fake with the facade, so the test can read what it was asked to do.
        struct Shared(Arc<FakeOs>);
        impl manage::Os for Shared {
            fn recycle(&self, dir: &RealDir) -> std::io::Result<()> {
                self.0.recycle(dir)
            }
            fn run(&self, c: &harness::ExternalCommand) -> std::io::Result<String> {
                self.0.run(c)
            }
        }

        fn open(h: &Home) -> (Skilltab, Arc<FakeOs>) {
            let os = Arc::new(FakeOs::default());
            let app = Skilltab::open_with(h.env(), h.p(".skilltab-data"), Box::new(Shared(os.clone()))).unwrap();
            (app, os)
        }

        fn mount_id(app: &Skilltab, entry: &Path) -> String {
            let inv = app.inventory();
            let m = inv.rows.iter().flat_map(|r| &r.mounts).find(|m| m.entry == entry.to_string_lossy());
            m.unwrap_or_else(|| panic!("no mount at {}", entry.display())).id.clone()
        }

        #[test]
        fn disable_shows_a_shelf_row_and_restore_brings_the_mount_back() {
            let h = Home::new();
            h.skill(".agents/skills/x", "x");
            h.junction(".claude/skills/x", ".agents/skills/x");
            let (app, _) = open(&h);
            let mount = mount_id(&app, &h.p(".claude/skills/x"));

            let plan = app.plan(&ActionRequest::Disable { mount }).unwrap();
            assert!(plan.ready);
            assert_eq!(plan.title, "Disable x in Claude Code");
            let report = app.apply(&plan.id).unwrap();
            assert_eq!(report.outcome, ApplyOutcomeKind::Applied);
            assert_eq!(report.steps[0].result, StepResultView::Done);

            let inv = app.inventory();
            assert_eq!(inv.revision, report.revision);
            assert_eq!(inv.shelf.len(), 1);
            assert_eq!(inv.shelf[0].name, "x");
            assert_eq!(inv.rows[0].mounts.len(), 1, "only Codex still mounts it");

            let again = app.apply(&plan.id);
            assert!(matches!(again, Err(CoreError::UnknownId { .. })), "a plan is used once");

            let restore = app.plan(&ActionRequest::Restore { shelf: inv.shelf[0].id.clone() }).unwrap();
            assert_eq!(app.apply(&restore.id).unwrap().outcome, ApplyOutcomeKind::Applied);
            let inv = app.inventory();
            assert!(inv.shelf.is_empty());
            assert_eq!(inv.rows[0].mounts.len(), 2);
        }

        #[test]
        fn a_plan_from_before_a_refresh_is_stale_and_nothing_runs() {
            let h = Home::new();
            h.skill(".claude/skills/r", "r");
            let (app, os) = open(&h);
            let skill = app.inventory().rows[0].id.clone();

            let plan = app.plan(&ActionRequest::DeleteBody { skill }).unwrap();
            app.refresh();
            let report = app.apply(&plan.id).unwrap();
            assert_eq!(report.outcome, ApplyOutcomeKind::Stale);
            assert!(os.recycled.lock().unwrap().is_empty());
            assert!(h.p(".claude/skills/r").exists());
        }

        #[test]
        fn a_jstack_synced_link_is_refused_with_the_exclude_remedy() {
            let h = Home::new();
            h.skill("jstack/skills/x", "x");
            h.junction(".claude/skills/x", "jstack/skills/x");
            std::fs::write(h.p(".jstack-links.txt"), format!("{}\n", h.p(".claude/skills/x").display())).unwrap();
            let (app, _) = open(&h);
            let mount = mount_id(&app, &h.p(".claude/skills/x"));

            let plan = app.plan(&ActionRequest::RemoveMount { mount }).unwrap();
            assert!(!plan.ready);
            assert_eq!(plan.blockers.len(), 1);
            assert!(matches!(plan.blockers[0].kind, BlockerKind::RecreatedBy));
            assert!(plan.blockers[0].remedy.contains("Add x to jstack/exclude.txt"), "{}", plan.blockers[0].remedy);
            assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Refused);
            assert!(h.p(".claude/skills/x").exists());
        }

        #[test]
        fn bundle_disable_hands_the_runner_exactly_the_claude_command() {
            let h = Home::new();
            h.claude_plugin();
            let (app, os) = open(&h);
            let bundle = app.inventory().bundles[0].id.clone();

            let plan = app.plan(&ActionRequest::BundleDisable { bundle }).unwrap();
            assert_eq!(plan.steps[0].detail, "claude plugin disable pstack@cursor-plugins");
            assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Applied);
            assert_eq!(*os.ran.lock().unwrap(), vec!["claude plugin disable pstack@cursor-plugins"]);
        }

        #[test]
        fn an_id_the_core_did_not_issue_is_rejected() {
            let h = Home::new();
            let (app, _) = open(&h);
            assert!(matches!(app.plan(&ActionRequest::Disable { mount: r"C:\x".into() }), Err(CoreError::UnknownId { .. })));
        }
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
