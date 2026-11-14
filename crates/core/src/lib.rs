//! skilltab-core. A plain Rust library with no Tauri dependency. The app crate is a thin shell over `Skilltab`.
//!
//! `Skilltab` is the facade. Callers see `view` types only, so the model can change without touching the shell.

mod aliases;
mod fsutil;
mod manage;
mod harness;
mod model;
mod projects;
mod provenance;
mod scan;
mod setup;
#[cfg(test)]
mod testutil;
mod usage;
pub mod view;

use std::path::{Path, PathBuf};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;
use view::{ActionRequest, AliasPick, AliasSuggestionView, ApplyOutcomeKind, ApplyReport, CoreError, Inventory, PlanView, RefreshReport, SkillDetail, Tallies};

pub use projects::discover_projects;

/// Read-only model access for the `dump` example. The app crate never needs these.
pub use model::{
    Bundle, BundleId, ContentHash, InstallScope, InvocationKey, Manifest, ManifestIssue, Mount, MountId,
    MountState, Origin, Ownership, Scope, ScanIssue, SkillBody, SkillId, Snapshot,
};
pub use model::{HarnessId, HarnessInfo, LinkPath, ProjectRoot, RealDir, Tally, TrackingSince};
pub use aliases::{apply as apply_aliases, key_from_wire, key_to_wire, suggest as suggest_aliases, Alias, AliasStore, RuleId, Suggestion};
pub use harness::InvocationKind;
pub use usage::{IngestProgress, IngestReport, UsageError, UsageIndex, UsagePoint};

pub struct Env {
    pub home: PathBuf,
    /// Project roots to check beyond the ones found in transcripts.
    pub projects: Vec<PathBuf>,
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
    /// Confirmed aliases already folded in.
    tallies: Tallies,
    aliases: AliasStore,
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
        let mut inventory = view::inventory(&c.snapshot, &c.usage.tallies, &c.usage.tracking_since, c.scan_ms);
        inventory.aliases = view::alias_views(&c.snapshot, &c.usage.aliases);
        inventory
    }

    /// Ghost usage keys that match exactly one installed skill once the old spelling is dropped.
    /// Nothing is linked until `confirm_alias`.
    pub fn alias_suggestions(&self) -> Vec<AliasSuggestionView> {
        let c = self.read();
        view::alias_suggestion_views(&c.snapshot, &aliases::suggest(&c.snapshot, &c.usage.tallies, &c.usage.aliases))
    }

    /// Counts usage under `from` for the installed skill that answers to `to`, then rescans so the
    /// totals and the revision move. Keys are the wire strings from `alias_suggestions`.
    pub fn confirm_alias(&self, harness: &str, from: &str, to: &str) -> Result<RefreshReport, CoreError> {
        self.confirm_aliases(&[AliasPick { harness: harness.to_owned(), from: from.to_owned(), to: to.to_owned() }])
    }

    /// `confirm_alias` for several keys under one rescan, so confirming a whole list is not one scan per key.
    /// Every pick is checked first, and one bad pick changes nothing.
    pub fn confirm_aliases(&self, picks: &[AliasPick]) -> Result<RefreshReport, CoreError> {
        let snap = self.read().snapshot;
        let mut aliases = Vec::with_capacity(picks.len());
        for pick in picks {
            let (harness, from, to) = parse_alias_ids(&pick.harness, &pick.from, &pick.to)?;
            if snap.resolve(harness, &from).is_some() {
                return Err(CoreError::Invalid { message: "that key still names an installed skill".to_owned() });
            }
            if snap.resolve(harness, &to).is_none() {
                return Err(CoreError::Invalid { message: "no installed skill answers to the target".to_owned() });
            }
            aliases.push(Alias { harness, from, to });
        }
        self.edit_aliases(|store| aliases.into_iter().fold(false, |changed, a| store.add(a) | changed))
    }

    /// Undoes a confirmed alias, so the old key reads as a ghost again.
    pub fn remove_alias(&self, harness: &str, from: &str) -> Result<RefreshReport, CoreError> {
        let (harness, from, _) = parse_alias_ids(harness, from, from)?;
        self.edit_aliases(|store| store.remove(harness, &from))
    }

    fn edit_aliases(&self, change: impl FnOnce(&mut AliasStore) -> bool) -> Result<RefreshReport, CoreError> {
        AliasStore::edit(&self.data_dir.join(ALIASES_FILE), change).map_err(|e| CoreError::Io { message: e.to_string() })?;
        Ok(self.refresh())
    }

    pub fn skill(&self, id: &str) -> Result<SkillDetail, CoreError> {
        let c = self.read();
        view::detail(&c.snapshot, id, &c.usage.tallies, &c.usage.tracking_since)
    }

    /// Opens the file manager on one hop of one mount's chain (0 is the entry, the last is the real folder).
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
        let c = self.read();
        let snap = c.snapshot;
        let action = view::parse_action(&snap, &self.env, self.os.as_ref(), request)?;
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

const ALIASES_FILE: &str = "aliases.json";

fn parse_alias_ids(harness: &str, from: &str, to: &str) -> Result<(model::HarnessId, model::InvocationKey, model::InvocationKey), CoreError> {
    let unknown = |id: &str| CoreError::UnknownId { id: id.to_owned() };
    Ok((
        model::HarnessId::parse(harness).ok_or_else(|| unknown(harness))?,
        aliases::key_from_wire(from).ok_or_else(|| unknown(from))?,
        aliases::key_from_wire(to).ok_or_else(|| unknown(to))?,
    ))
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn build(env: &Env, data_dir: &Path, shelved: &[model::ShelfEntry], revision: u64) -> Current {
    let started = Instant::now();
    let snapshot = scan_disk(env, shelved, revision);
    let usage = load_usage(env, data_dir);
    Current { snapshot: Arc::new(snapshot), usage: Arc::new(usage), scan_ms: started.elapsed().as_millis() as u32 }
}

/// One scan of every harness root, with no usage. Shared by the facade and by tests that only need a snapshot.
pub(crate) fn scan_disk(env: &Env, shelved: &[model::ShelfEntry], revision: u64) -> Snapshot {
    let projects = scan::project_roots(&project_candidates(env));
    let provenance = provenance::Provenance::load(env);
    scan::scan(env, harness::registry(), &projects, &provenance, shelved.to_vec(), revision)
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
        let aliases = AliasStore::load(&data_dir.join(ALIASES_FILE)).unwrap_or_else(|e| {
            eprintln!("skilltab: aliases not applied: {e}");
            AliasStore::default()
        });
        Ok(Usage { tallies: aliases::apply(index.totals(chrono::Utc::now())?, &aliases), aliases, tracking_since: index.tracking_since()? })
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

#[cfg(target_os = "macos")]
fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("open").arg("-R").arg(path).spawn().map(|_| ())
}

/// xdg-open has no select verb, so the folder that holds `path` opens instead.
#[cfg(all(unix, not(target_os = "macos")))]
fn reveal_in_explorer(path: &Path) -> std::io::Result<()> {
    std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn().map(|_| ())
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
        let env = Env { home: home.to_path_buf(), projects: Vec::new() };
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
        use crate::manage::tests::FakeOs;
        use crate::testutil::Home;
        use std::sync::Arc;
        use view::{ActionRequest, ApplyOutcomeKind, StepResultView};

        /// Shares the fake with the facade, so the test can read what it was asked to do.
        struct Shared(Arc<FakeOs>);
        impl manage::Os for Shared {
            fn recycle(&self, dir: &RealDir) -> std::io::Result<()> {
                self.0.recycle(dir)
            }
            fn run(&self, c: &harness::ExternalCommand) -> std::io::Result<String> {
                self.0.run(c)
            }
            fn find_tool(&self, program: &str) -> bool {
                self.0.find_tool(program)
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
            h.link(".claude/skills/x", ".agents/skills/x");
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

        fn bulk_ids(app: &Skilltab, names: &[&str]) -> Vec<String> {
            let inv = app.inventory();
            names.iter().map(|n| inv.rows.iter().find(|r| r.name == *n).unwrap_or_else(|| panic!("no row {n}")).id.clone()).collect()
        }

        #[test]
        fn a_bulk_delete_names_the_count_lists_what_it_skips_and_recycles_each_folder_once() {
            let h = Home::new();
            h.skill(".claude/skills/a", "a");
            h.skill(".claude/skills/c", "c");
            h.claude_plugin();
            let (app, os) = open(&h);
            let skills = bulk_ids(&app, &["a", "architect", "c", "a"]);

            let plan = app.plan(&ActionRequest::DeleteBodies { skills }).unwrap();
            assert_eq!((plan.title.as_str(), plan.confirm.as_str()), ("Delete 2 skills everywhere", "Delete 2 skills"));
            assert!(plan.ready && plan.blockers.is_empty());
            assert_eq!(plan.steps.len(), 2, "the repeated id is one skill");
            assert!(plan.steps.iter().all(|s| s.text.contains(manage::TRASH_NAME)), "{:?}", plan.steps.iter().map(|s| &s.text).collect::<Vec<_>>());
            assert_eq!(app.inventory().trash_name, manage::TRASH_NAME);
            let skipped: Vec<(&str, bool)> = plan.skipped.iter().map(|s| (s.name.as_str(), s.reason.contains("plugin"))).collect();
            assert_eq!(skipped, [("architect", true)]);
            assert!(plan.skipped[0].remedy.contains("Disable plugin"), "{}", plan.skipped[0].remedy);

            let report = app.apply(&plan.id).unwrap();
            assert_eq!(report.outcome, ApplyOutcomeKind::Applied);
            assert_eq!(os.recycled.lock().unwrap().len(), 2);
            let left: Vec<String> = app.inventory().rows.into_iter().map(|r| r.name).collect();
            assert_eq!(left, ["architect"]);

            let bad = ActionRequest::DeleteBodies { skills: vec!["not-a-folder-id".into()] };
            assert!(matches!(app.plan(&bad), Err(CoreError::UnknownId { .. })));
        }

        #[test]
        fn a_bulk_delete_with_nothing_deletable_is_not_ready() {
            let h = Home::new();
            h.claude_plugin();
            let (app, _) = open(&h);

            let empty = app.plan(&ActionRequest::DeleteBodies { skills: Vec::new() }).unwrap();
            assert!(!empty.ready && empty.skipped.is_empty() && empty.title == "Delete skills everywhere");

            let all = app.plan(&ActionRequest::DeleteBodies { skills: bulk_ids(&app, &["architect"]) }).unwrap();
            assert!(!all.ready && all.steps.is_empty() && all.blockers.is_empty());
            assert_eq!(all.skipped.len(), 1);
            assert_eq!(app.apply(&all.id).unwrap().steps.len(), 0);

            let single = app.plan(&ActionRequest::DeleteBody { skill: bulk_ids(&app, &["architect"]).remove(0) }).unwrap();
            assert!(single.skipped.is_empty() && !single.blockers.is_empty(), "a single delete still reports a blocker");
        }

        fn fixture(rel: &str) -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/setup").join(rel)
        }

        /// A copy of the fixture home, and a setup folder at `~/repo` that already holds the two own skills.
        fn export_home() -> Home {
            let h = Home::new();
            crate::testutil::copy_tree(&fixture("home"), &h.root);
            crate::testutil::copy_tree(&fixture("expected/skills"), &h.p("repo/skills"));
            h
        }

        fn export_request(h: &Home) -> ActionRequest {
            ActionRequest::ExportSetup { folder: h.p("repo").to_string_lossy().into_owned() }
        }

        /// Every file under `root`, by `/` path.
        fn tree(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
            fn walk(dir: &Path, rel: &str, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
                for item in std::fs::read_dir(dir).unwrap() {
                    let item = item.unwrap();
                    let rel = format!("{rel}{}", item.file_name().to_string_lossy());
                    if item.path().is_dir() {
                        walk(&item.path(), &format!("{rel}/"), out);
                    } else {
                        out.insert(rel, std::fs::read(item.path()).unwrap());
                    }
                }
            }
            let mut out = std::collections::BTreeMap::new();
            walk(root, "", &mut out);
            out
        }

        fn notes_of(plan: &view::PlanView, kind: view::NoteKind) -> Vec<String> {
            plan.notes.iter().filter(|n| n.kind == kind).map(|n| n.text.clone()).collect()
        }

        #[test]
        fn export_writes_the_bytes_backup_ps1_wrote_and_a_second_export_has_nothing_to_do() {
            let h = export_home();
            let (app, os) = open(&h);

            let plan = app.plan(&export_request(&h)).unwrap();
            assert_eq!(plan.title, format!("Export setup to {}", view::path_str(&h.p("repo"))));
            assert!(plan.ready && plan.blockers.is_empty(), "{:?}", plan.blockers.iter().map(|b| &b.reason).collect::<Vec<_>>());
            assert_eq!(plan.steps.len(), 5);
            assert!(plan.steps.iter().all(|s| s.kind == view::StepKind::Write && s.text == "Write a new file"));
            let attention = notes_of(&plan, view::NoteKind::Attention);
            assert_eq!(attention.len(), 1);
            assert!(attention[0].contains("local-market") && attention[0].contains("directory"), "{attention:?}");

            let report = app.apply(&plan.id).unwrap();
            assert_eq!(report.outcome, ApplyOutcomeKind::Applied);
            assert!(report.steps.iter().all(|s| s.result == StepResultView::Done));

            let written = std::fs::read(h.p("repo/manifest.json")).unwrap();
            assert_eq!(written, std::fs::read(fixture("expected/manifest.json")).unwrap());
            assert_eq!(tree(&h.p("repo/config")), tree(&fixture("expected/config")));
            assert!(os.ran.lock().unwrap().is_empty() && os.recycled.lock().unwrap().is_empty());

            let again = app.plan(&export_request(&h)).unwrap();
            assert!(!again.ready && again.steps.is_empty() && again.blockers.is_empty());
            assert_eq!(notes_of(&again, view::NoteKind::Present).len(), 5);
            assert_eq!(notes_of(&again, view::NoteKind::Attention).len(), 1, "the marketplace the format cannot rebuild stays named");
        }

        #[test]
        fn export_takes_a_home_relative_folder_and_refuses_one_that_is_not_a_full_path() {
            let h = export_home();
            let (app, _) = open(&h);

            let plan = app.plan(&ActionRequest::ExportSetup { folder: "~/repo".into() }).unwrap();
            assert!(plan.ready);
            assert_eq!(plan.title, format!("Export setup to {}", view::path_str(&h.p("repo"))));

            for folder in ["repo", "~/missing"] {
                let e = app.plan(&ActionRequest::ExportSetup { folder: folder.into() });
                assert!(matches!(e, Err(CoreError::Invalid { .. })), "{folder}");
            }
        }

        #[test]
        fn export_never_reaches_settings_config_hooks_or_auth_files() {
            let h = export_home();
            for rel in [".claude/hooks.json", ".claude/auth.json", ".codex/auth.json", ".codex/hooks.json"] {
                std::fs::write(h.p(rel), "{\"token\":\"x\"}").unwrap();
            }
            let (app, _) = open(&h);

            let plan = app.plan(&export_request(&h)).unwrap();
            let denied = ["settings.json", "config.toml", "hooks.json", "auth.json", "notes.txt"];
            for step in &plan.steps {
                assert!(denied.iter().all(|n| !step.detail.contains(n)), "{}", step.detail);
                assert!(step.detail.starts_with(&view::path_str(&h.p("repo"))), "{}", step.detail);
            }
            assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Applied);
            let top: Vec<String> = tree(&h.p("repo")).keys().map(|k| k.split('/').next().unwrap().to_owned()).collect();
            assert!(top.iter().all(|t| ["config", "manifest.json", "skills"].contains(&t.as_str())), "{top:?}");
        }

        #[test]
        fn a_secret_in_an_instruction_file_blocks_the_whole_export_and_names_the_file_not_the_secret() {
            let h = export_home();
            let secret = format!("sk-{}", "a1B2".repeat(6));
            std::fs::write(h.p(".claude/CLAUDE.md"), format!("# Notes\nkey: {secret}\n")).unwrap();
            let (app, _) = open(&h);

            let plan = app.plan(&export_request(&h)).unwrap();
            assert!(!plan.ready && plan.steps.is_empty());
            assert_eq!(plan.blockers.len(), 1);
            let reason = &plan.blockers[0].reason;
            assert!(reason.contains("~/.claude/CLAUDE.md") && reason.contains("sk-["), "{reason}");
            assert!(!reason.contains(&secret) && !plan.blockers[0].remedy.contains(&secret));

            assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Refused);
            assert!(!h.p("repo/manifest.json").exists() && !h.p("repo/config").exists());
        }

        #[test]
        fn a_secret_in_a_utf_16_file_blocks_the_export_as_backup_ps1_refuses_it() {
            let h = export_home();
            let secret = format!("ghp_{}", "a1B2".repeat(6));
            let mut bytes = vec![0xff, 0xfe];
            bytes.extend(format!("# Rules
token: {secret}
").encode_utf16().flat_map(u16::to_le_bytes));
            std::fs::write(h.p(".claude/rules/a-rule.md"), bytes).unwrap();
            let (app, _) = open(&h);

            let plan = app.plan(&export_request(&h)).unwrap();
            assert!(!plan.ready && plan.steps.is_empty());
            let reasons: Vec<&String> = plan.blockers.iter().map(|b| &b.reason).collect();
            assert!(reasons.iter().any(|r| r.contains("~/.claude/rules/a-rule.md")), "{reasons:?}");
            assert!(reasons.iter().all(|r| !r.contains(&secret)));
            assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Refused);
            assert!(!h.p("repo/manifest.json").exists());
        }

        #[test]
        fn an_unreadable_or_unsupported_skill_lock_blocks_the_export() {
            for lock in [r#"{"version":2,"skills":{}}"#, "{not json"] {
                let h = export_home();
                std::fs::write(h.p(".agents/.skill-lock.json"), lock).unwrap();
                let (app, _) = open(&h);

                let plan = app.plan(&export_request(&h)).unwrap();
                assert!(!plan.ready && plan.steps.is_empty(), "{lock}");
                assert!(plan.blockers.iter().any(|b| b.reason.contains(".skill-lock.json")), "{lock}");
                assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Refused);
                assert!(!h.p("repo/manifest.json").exists(), "no manifest from half a machine");
            }
        }

        #[test]
        fn an_unreadable_plugin_record_blocks_the_export_too() {
            let h = export_home();
            std::fs::write(h.p(".claude/settings.json"), "{oops").unwrap();
            let (app, _) = open(&h);
            let plan = app.plan(&export_request(&h)).unwrap();
            assert!(!plan.ready && plan.blockers.iter().any(|b| b.reason.contains("settings.json")));
        }

        #[test]
        fn a_setup_folder_inside_a_harness_folder_blocks_the_export() {
            let h = export_home();
            std::fs::create_dir_all(h.p(".claude/backup")).unwrap();
            let (app, _) = open(&h);
            let plan = app.plan(&ActionRequest::ExportSetup { folder: "~/.claude/backup".into() }).unwrap();
            assert!(!plan.ready && plan.blockers.iter().any(|b| b.reason.contains("inside ~/.claude")), "{:?}", plan.blockers.iter().map(|b| &b.reason).collect::<Vec<_>>());
        }

        #[test]
        fn a_link_in_the_setup_folder_blocks_the_export_and_nothing_behind_it_is_touched() {
            let places = ["manifest.json", "config", "config/claude", "config/claude/rules", "config/claude/rules/z.md"];
            for place in places {
                let h = export_home();
                let (app, os) = open(&h);
                let first = app.plan(&export_request(&h)).unwrap();
                app.apply(&first.id).unwrap();

                std::fs::create_dir_all(h.p("outside")).unwrap();
                for name in ["old.md", "CLAUDE.md"] {
                    std::fs::write(h.p("outside").join(name), "outside").unwrap();
                }
                let linked = h.p(&format!("repo/{place}"));
                if linked.is_dir() {
                    std::fs::remove_dir_all(&linked).unwrap();
                } else if linked.exists() {
                    std::fs::remove_file(&linked).unwrap();
                }
                h.link(&format!("repo/{place}"), "outside");
                let before = crate::testutil::listing(&h.p("outside"));

                let plan = app.plan(&export_request(&h)).unwrap();
                let steps: Vec<&String> = plan.steps.iter().map(|s| &s.detail).collect();
                let reasons: Vec<&String> = plan.blockers.iter().map(|b| &b.reason).collect();
                assert!(!plan.ready && steps.is_empty(), "{place}: {steps:?}");
                assert!(reasons.iter().any(|r| r.contains(place) && r.contains("is a link")), "{place}: {reasons:?}");
                assert!(plan.blockers.iter().all(|b| b.remedy.contains("Remove the link or pick another folder")), "{place}");

                assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Refused, "{place}");
                assert_eq!(crate::testutil::listing(&h.p("outside")), before, "{place}");
                assert!(os.ran.lock().unwrap().is_empty() && os.recycled.lock().unwrap().is_empty());
            }
        }

        #[test]
        fn export_replaces_changed_copies_and_deletes_copies_of_files_that_left_the_machine() {
            let h = export_home();
            let (app, _) = open(&h);
            let first = app.plan(&export_request(&h)).unwrap();
            app.apply(&first.id).unwrap();
            std::fs::write(h.p("repo/config/claude/other.json"), "{}").unwrap();
            std::fs::write(h.p("repo/config/claude/rules/old.txt"), "kept").unwrap();

            std::fs::remove_file(h.p(".claude/rules/b-rule.md")).unwrap();
            std::fs::write(h.p(".claude/rules/a-rule.md"), "rule a, edited\n").unwrap();
            let plan = app.plan(&export_request(&h)).unwrap();

            let steps: Vec<(view::StepKind, String)> = plan.steps.iter().map(|s| (s.kind, s.detail.replace('\\', "/"))).collect();
            let repo = view::path_str(&h.p("repo")).replace('\\', "/");
            assert_eq!(
                steps,
                [
                    (view::StepKind::Write, format!("{repo}/config/claude/rules/a-rule.md")),
                    (view::StepKind::Delete, format!("{repo}/config/claude/rules/b-rule.md")),
                ]
            );
            assert_eq!(plan.steps[0].text, "Replace the file");
            assert_eq!(app.apply(&plan.id).unwrap().outcome, ApplyOutcomeKind::Applied);

            let config = tree(&h.p("repo/config"));
            assert_eq!(config["claude/rules/a-rule.md"], b"rule a, edited\n");
            assert!(!config.contains_key("claude/rules/b-rule.md"));
            assert!(config.contains_key("claude/other.json") && config.contains_key("claude/rules/old.txt"), "files outside the allowlist shape stay");
            let again = app.plan(&export_request(&h)).unwrap();
            assert!(again.steps.is_empty());
        }

        #[test]
        fn a_manifest_that_appears_after_planning_is_not_overwritten() {
            let h = export_home();
            let (app, _) = open(&h);
            let plan = app.plan(&export_request(&h)).unwrap();
            std::fs::write(h.p("repo/manifest.json"), "{}").unwrap();

            let report = app.apply(&plan.id).unwrap();
            assert_eq!(report.outcome, ApplyOutcomeKind::Stale);
            assert!(report.reasons.iter().any(|r| r.contains("manifest.json")), "{:?}", report.reasons);
            assert_eq!(std::fs::read(h.p("repo/manifest.json")).unwrap(), b"{}");
            assert!(!h.p("repo/config").exists());
        }

        #[test]
        fn an_id_the_core_did_not_issue_is_rejected() {
            let h = Home::new();
            let (app, _) = open(&h);
            assert!(matches!(app.plan(&ActionRequest::Disable { mount: r"C:\x".into() }), Err(CoreError::UnknownId { .. })));
        }
    }

    #[test]
    fn confirming_an_alias_moves_the_ghost_history_onto_the_skill_and_removing_it_moves_it_back() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        let projects = tmp.path().join(".claude/projects/p");
        fs::create_dir_all(&projects).unwrap();
        let slash = |uuid: &str, name: &str| {
            serde_json::json!({"type": "user", "uuid": uuid, "sessionId": "s", "cwd": "D:\\p", "timestamp": "2026-11-01T10:00:00.000Z",
                "message": {"content": format!("<command-name>/{name}</command-name>")}})
            .to_string()
        };
        let lines = [slash("u1", "mattpocock-skills:tdd"), slash("u2", "mattpocock-skills:tdd"), slash("u3", "compact"), slash("u4", "tdd")];
        fs::write(projects.join("a.jsonl"), lines.join("\n") + "\n").unwrap();

        let app = open(tmp.path());
        let total = |app: &Skilltab| app.inventory().rows[0].usage.total;
        assert_eq!(total(&app), 1);
        let suggestions = app.alias_suggestions();
        assert_eq!(suggestions.len(), 1, "compact is a built-in and gets none");
        let s = &suggestions[0];
        assert_eq!((s.from.as_str(), s.to.as_str(), s.events), ("name:mattpocock-skills:tdd", "name:tdd", 2));
        assert!(app.inventory().aliases.is_empty(), "suggesting links nothing");

        assert!(matches!(app.confirm_alias("claude-code", "name:tdd", "name:tdd"), Err(CoreError::Invalid { .. })), "a live key is not a ghost");
        assert!(matches!(app.confirm_alias("claude-code", &s.from, "name:nothing"), Err(CoreError::Invalid { .. })));
        assert!(matches!(app.confirm_alias("claude-code", "bogus", &s.to), Err(CoreError::UnknownId { .. })));
        assert!(matches!(app.confirm_alias("nope", &s.from, &s.to), Err(CoreError::UnknownId { .. })));
        assert_eq!(total(&app), 1);

        let before = app.inventory().revision;
        assert_eq!(app.confirm_alias("claude-code", &s.from, &s.to).unwrap().revision, before + 1);
        assert_eq!(total(&app), 3);
        assert!(app.alias_suggestions().is_empty());
        let inv = app.inventory();
        assert_eq!(inv.aliases.len(), 1);
        assert_eq!(inv.aliases[0].target_body.as_deref(), Some(inv.rows[0].id.as_str()));
        assert!(tmp.path().join("data/aliases.json").exists());

        assert_eq!(total(&open(tmp.path())), 3, "the confirmation survives a restart");

        app.remove_alias("claude-code", &s.from).unwrap();
        assert_eq!(total(&app), 1);
        assert_eq!(app.alias_suggestions().len(), 1);
        assert!(app.inventory().aliases.is_empty());
    }

    #[test]
    fn confirming_a_list_of_aliases_is_one_rescan_and_one_bad_pick_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        skill(tmp.path(), ".claude/skills/grill", "grill");
        let projects = tmp.path().join(".claude/projects/p");
        fs::create_dir_all(&projects).unwrap();
        let slash = |uuid: &str, name: &str| {
            serde_json::json!({"type": "user", "uuid": uuid, "sessionId": "s", "cwd": "D:\\p", "timestamp": "2026-11-01T10:00:00.000Z",
                "message": {"content": format!("<command-name>/{name}</command-name>")}})
            .to_string()
        };
        let lines = [slash("u1", "old:tdd"), slash("u2", "old:tdd"), slash("u3", "old:grill")];
        fs::write(projects.join("a.jsonl"), lines.join("\n") + "\n").unwrap();

        let app = open(tmp.path());
        let picks: Vec<AliasPick> = app
            .alias_suggestions()
            .iter()
            .map(|s| AliasPick { harness: s.harness.id.clone(), from: s.from.clone(), to: s.to.clone() })
            .collect();
        assert_eq!(picks.len(), 2);

        let mut bad = picks.clone();
        bad.push(AliasPick { harness: "claude-code".into(), from: "name:old:nothing".into(), to: "name:missing".into() });
        assert!(matches!(app.confirm_aliases(&bad), Err(CoreError::Invalid { .. })));
        assert!(app.inventory().aliases.is_empty());

        let before = app.inventory().revision;
        assert_eq!(app.confirm_aliases(&picks).unwrap().revision, before + 1);
        let inv = app.inventory();
        assert_eq!(inv.aliases.len(), 2);
        assert!(app.alias_suggestions().is_empty());
        let total = |name: &str| inv.rows.iter().find(|r| r.name == name).map(|r| r.usage.total);
        assert_eq!((total("tdd"), total("grill")), (Some(2), Some(1)));
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
