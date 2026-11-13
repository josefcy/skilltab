use super::*;
use crate::fsutil::strip_verbatim;
use crate::provenance::Provenance;
use crate::Env;
use std::fs;
use std::process::Command;
use std::sync::Mutex;

pub(crate) struct Home {
    _guard: tempfile::TempDir,
    pub root: PathBuf,
}

impl Home {
    pub fn new() -> Home {
        let guard = tempfile::tempdir().unwrap();
        let root = strip_verbatim(fs::canonicalize(guard.path()).unwrap());
        Home { _guard: guard, root }
    }
    pub fn p(&self, rel: &str) -> PathBuf {
        self.root.join(rel.replace('/', "\\"))
    }
    pub fn skill(&self, rel: &str, name: &str) {
        let dir = self.p(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nbody of {name}\n")).unwrap();
    }
    pub fn junction(&self, link: &str, target: &str) {
        let link = self.p(link);
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        let out = Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(self.p(target)).output().unwrap();
        assert!(out.status.success(), "mklink: {}", String::from_utf8_lossy(&out.stderr));
    }
    /// A Claude plugin `pstack@cursor-plugins` with one skill, enabled in user settings.
    pub fn claude_plugin(&self) {
        let root = self.p(".claude/plugins/cache/cursor-plugins/pstack/1.0.0");
        self.skill(".claude/plugins/cache/cursor-plugins/pstack/1.0.0/skills/architect", "architect");
        let installed = serde_json::json!({"version": 2, "plugins": {"pstack@cursor-plugins": [
            {"scope": "user", "installPath": root, "version": "1.0.0"}]}});
        fs::write(self.p(".claude/plugins/installed_plugins.json"), installed.to_string()).unwrap();
        fs::write(self.p(".claude/settings.json"), r#"{"enabledPlugins":{"pstack@cursor-plugins":true}}"#).unwrap();
    }
    pub fn env(&self) -> Env {
        Env { home: self.root.clone(), projects: vec![], jstack_root: Some(self.p("jstack")) }
    }
    fn shelf(&self) -> Shelf {
        Shelf::open(&self.p(".skilltab-data")).unwrap()
    }
    fn scan(&self, shelf: &Shelf, revision: u64) -> Snapshot {
        let env = self.env();
        let prov = Provenance::load(&env, env.jstack_root.as_deref());
        crate::scan::scan(&env, registry(), &[], &prov, shelf.entries().to_vec(), revision)
    }
}

/// Records instead of recycling or running, and moves a "recycled" folder aside so the step's end state holds.
#[derive(Default)]
pub(crate) struct FakeOs {
    pub recycled: Mutex<Vec<PathBuf>>,
    pub ran: Mutex<Vec<String>>,
}

impl Os for FakeOs {
    fn recycle(&self, dir: &RealDir) -> io::Result<()> {
        let mut recycled = self.recycled.lock().unwrap();
        recycled.push(dir.as_path().to_path_buf());
        let bin = dir.as_path().parent().unwrap().join(format!(".bin-{}", recycled.len()));
        fs::rename(dir.as_path(), bin)
    }
    fn run(&self, command: &ExternalCommand) -> io::Result<String> {
        self.ran.lock().unwrap().push(command.display());
        Ok("ok".into())
    }
}

fn mount<'a>(snap: &'a Snapshot, harness: HarnessId, entry: &Path) -> &'a Mount {
    snap.mounts
        .values()
        .find(|m| m.harness == harness && norm_key(&m.entry) == norm_key(entry))
        .unwrap_or_else(|| panic!("no {harness:?} mount at {}", entry.display()))
}

fn results(outcome: ApplyOutcome) -> Vec<StepResult> {
    match outcome {
        ApplyOutcome::Applied { results, .. } => results,
        other => panic!("expected Applied, got {other:?}"),
    }
}

#[test]
fn removing_a_junction_mount_leaves_its_target_and_can_be_restored() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.junction(".claude/skills/x", ".agents/skills/x");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let claude = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));

    let p = plan(&snap, &shelf, ManageAction::RemoveMount(claude.id.clone()));
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert!(matches!(p.steps.as_slice(), [Step::RemoveLink { .. }]));
    assert_eq!(p.effects, vec![Effect::HarnessLosesSkill { mount: claude.id.clone() }]);
    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Done]);

    assert!(!h.p(".claude/skills/x").exists());
    assert!(h.p(".agents/skills/x/SKILL.md").is_file(), "the target survives");
    assert_eq!(shelf.entries().len(), 1);
    assert_eq!(shelf.entries()[0].reason, ShelfReason::Removed);
}

#[test]
fn removing_a_middle_hop_lists_every_mount_it_breaks() {
    let h = Home::new();
    h.skill("elsewhere/y", "y");
    h.junction(".agents/skills/y", "elsewhere/y");
    h.junction(".claude/skills/y", ".agents/skills/y");
    let shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let codex = mount(&snap, HarnessId::CODEX, &h.p(".agents/skills/y"));
    let claude = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/y"));

    let p = plan(&snap, &shelf, ManageAction::RemoveMount(codex.id.clone()));
    assert!(p.effects.contains(&Effect::AlsoBreaks { mount: claude.id.clone(), via: h.p(".agents/skills/y") }), "{:?}", p.effects);
}

#[test]
fn delete_body_removes_every_link_then_recycles_the_real_folder() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.junction(".claude/skills/x", ".agents/skills/x");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let codex = mount(&snap, HarnessId::CODEX, &h.p(".agents/skills/x"));
    let claude = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));

    let only_codex = plan(&snap, &shelf, ManageAction::RemoveMount(codex.id.clone()));
    assert_eq!(only_codex.blockers, vec![Blocker::SharedBody { dependents: vec![claude.id.clone()] }]);

    let p = plan(&snap, &shelf, ManageAction::DeleteBody(codex.body.clone()));
    assert!(p.blockers.is_empty(), "{:?}", p.blockers);
    assert!(matches!(p.steps.as_slice(), [Step::RemoveLink { .. }, Step::Recycle { .. }]), "{:?}", p.steps);
    assert!(p.effects.contains(&Effect::BodyRecycled { dir: h.p(".agents/skills/x") }));
    let os = FakeOs::default();
    assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Done, StepResult::Done]);

    assert_eq!(*os.recycled.lock().unwrap(), vec![h.p(".agents/skills/x")]);
    assert!(!h.p(".claude/skills/x").exists() && !h.p(".agents/skills/x").exists());
    assert!(shelf.entries().is_empty(), "a deleted skill is not shelved");
}

#[test]
#[ignore = "puts a temp folder in the real Recycle Bin; run with --ignored"]
fn recycle_really_moves_a_folder_to_the_recycle_bin() {
    let h = Home::new();
    h.skill(".claude/skills/bin-me", "bin-me");
    let snap = h.scan(&h.shelf(), 1);
    let body = snap.bodies.values().next().unwrap();

    SystemOs.recycle(&body.dir).unwrap();
    assert!(!body.dir.as_path().exists());
}

#[test]
fn disable_then_restore_round_trips_a_link() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.junction(".claude/skills/x", ".agents/skills/x");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let claude = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));

    let off = plan(&snap, &shelf, ManageAction::Disable(claude.id.clone()));
    results(apply(&off, &mut shelf, &FakeOs::default()));
    let snap = h.scan(&shelf, 2);
    assert_eq!(snap.mounts.len(), 1, "only Codex still sees it");
    assert_eq!(snap.shelf.len(), 1);
    assert_eq!(snap.shelf[0].reason, ShelfReason::Disabled);

    let on = plan(&snap, &shelf, ManageAction::Restore(snap.shelf[0].id.clone()));
    assert!(on.blockers.is_empty(), "{:?}", on.blockers);
    assert_eq!(results(apply(&on, &mut shelf, &FakeOs::default())), vec![StepResult::Done]);

    assert!(still_link_to(&h.p(".claude/skills/x"), &h.p(".agents/skills/x")));
    assert!(shelf.entries().is_empty());
    let snap = h.scan(&shelf, 3);
    assert_eq!(snap.mounts.len(), 2);
    assert_eq!(snap.bodies.len(), 1, "the restored link reaches the same folder");
}

#[test]
fn disable_then_restore_round_trips_a_real_folder() {
    let h = Home::new();
    h.skill(".claude/skills/r", "r");
    fs::write(h.p(".claude/skills/r/notes.txt"), "keep me").unwrap();
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/r"));
    let before = snap.bodies[&m.body].content_hash.clone();

    let off = plan(&snap, &shelf, ManageAction::Disable(m.id.clone()));
    assert!(matches!(off.steps.as_slice(), [Step::Stash { .. }]));
    results(apply(&off, &mut shelf, &FakeOs::default()));
    assert!(!h.p(".claude/skills/r").exists());
    let stash = shelf.stash_path(&shelf.entries()[0].id);
    assert!(stash.join("notes.txt").is_file(), "the folder sits in the shelf");

    let snap = h.scan(&shelf, 2);
    assert!(snap.mounts.is_empty());
    let on = plan(&snap, &shelf, ManageAction::Restore(snap.shelf[0].id.clone()));
    results(apply(&on, &mut shelf, &FakeOs::default()));

    let snap = h.scan(&shelf, 3);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/r"));
    assert_eq!(snap.bodies[&m.body].content_hash, before);
    assert!(!stash.exists() && shelf.entries().is_empty());
}

#[test]
fn a_jstack_synced_link_is_blocked_and_left_alone() {
    let h = Home::new();
    h.skill("jstack/skills/x", "x");
    h.junction(".claude/skills/x", "jstack/skills/x");
    fs::write(h.p(".jstack-links.txt"), format!("{}\r\n", h.p(".claude/skills/x").display())).unwrap();
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));

    for action in [ManageAction::RemoveMount(m.id.clone()), ManageAction::Disable(m.id.clone())] {
        let p = plan(&snap, &shelf, action);
        assert_eq!(p.blockers, vec![Blocker::RecreatedBy { owner: Ownership::JstackSync, name: "x".into() }]);
        assert!(matches!(apply(&p, &mut shelf, &FakeOs::default()), ApplyOutcome::Refused));
    }
    let delete = plan(&snap, &shelf, ManageAction::DeleteBody(m.body.clone()));
    assert!(delete.blockers.contains(&Blocker::InsideJstackRepo));
    assert!(h.p(".claude/skills/x").exists());
}

#[test]
fn a_bundle_member_is_blocked_and_the_bundle_action_runs_the_harness_cli() {
    let h = Home::new();
    h.claude_plugin();
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = snap.mounts.values().next().unwrap();
    let Scope::Bundle { bundle } = &m.scope else { panic!("{:?}", m.scope) };

    for action in [ManageAction::Disable(m.id.clone()), ManageAction::RemoveMount(m.id.clone()), ManageAction::DeleteBody(m.body.clone())] {
        let p = plan(&snap, &shelf, action);
        assert!(p.blockers.contains(&Blocker::PartOfBundle { bundle: bundle.clone() }), "{:?}", p.blockers);
    }

    let already = plan(&snap, &shelf, ManageAction::Bundle(bundle.clone(), BundleOp::Enable));
    assert!(already.steps.is_empty() && already.blockers.is_empty());

    let off = plan(&snap, &shelf, ManageAction::Bundle(bundle.clone(), BundleOp::Disable));
    assert_eq!(off.effects, vec![Effect::HarnessLosesSkill { mount: m.id.clone() }]);
    let os = FakeOs::default();
    let ApplyOutcome::Applied { results, output } = apply(&off, &mut shelf, &os) else { panic!() };
    assert_eq!(results, vec![StepResult::Done]);
    assert_eq!(output, vec!["ok"]);
    assert_eq!(*os.ran.lock().unwrap(), vec!["claude plugin disable pstack@cursor-plugins"]);
}

#[test]
fn a_plan_whose_link_target_changed_is_refused_as_stale() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.skill("other/x", "x");
    h.junction(".claude/skills/x", ".agents/skills/x");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));
    let p = plan(&snap, &shelf, ManageAction::RemoveMount(m.id.clone()));

    fs::remove_dir(h.p(".claude/skills/x")).unwrap();
    h.junction(".claude/skills/x", "other/x");

    let ApplyOutcome::Stale { reasons } = apply(&p, &mut shelf, &FakeOs::default()) else { panic!("expected Stale") };
    assert_eq!(reasons.len(), 1, "{reasons:?}");
    assert!(still_link_to(&h.p(".claude/skills/x"), &h.p("other/x")), "nothing ran");
    assert!(shelf.entries().is_empty());
}

#[test]
fn a_real_folder_edited_after_planning_is_not_recycled() {
    let h = Home::new();
    h.skill(".claude/skills/r", "r");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/r"));
    let p = plan(&snap, &shelf, ManageAction::DeleteBody(m.body.clone()));

    fs::write(h.p(".claude/skills/r/new.md"), "edited").unwrap();
    let os = FakeOs::default();
    assert!(matches!(apply(&p, &mut shelf, &os), ApplyOutcome::Stale { .. }));
    assert!(os.recycled.lock().unwrap().is_empty());
}

#[test]
fn applying_a_plan_twice_converges() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.junction(".claude/skills/x", ".agents/skills/x");
    h.skill(".claude/skills/r", "r");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let link = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));
    let real = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/r"));
    let os = FakeOs::default();

    for p in [
        plan(&snap, &shelf, ManageAction::Disable(link.id.clone())),
        plan(&snap, &shelf, ManageAction::Disable(real.id.clone())),
    ] {
        assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Done]);
        assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Skipped]);
    }
    assert_eq!(shelf.entries().len(), 2);

    let snap = h.scan(&shelf, 2);
    for e in snap.shelf.clone() {
        let p = plan(&snap, &shelf, ManageAction::Restore(e.id.clone()));
        assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Done]);
        assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Skipped]);
    }
    assert!(shelf.entries().is_empty());
    assert_eq!(h.scan(&shelf, 3).mounts.len(), 3);
}

#[test]
fn shelf_survives_a_reopen_and_refuses_a_corrupt_file() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.junction(".claude/skills/x", ".agents/skills/x");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));
    results(apply(&plan(&snap, &shelf, ManageAction::Disable(m.id.clone())), &mut shelf, &FakeOs::default()));

    let again = h.shelf();
    assert_eq!(again.entries().len(), 1);
    assert_eq!(again.entries()[0].name, "x");
    assert!(matches!(&again.entries()[0].held, Held::Link { target } if norm_key(target) == norm_key(&h.p(".agents/skills/x"))));

    fs::write(h.p(".skilltab-data/shelf.json"), "{not json").unwrap();
    assert!(Shelf::open(&h.p(".skilltab-data")).is_err());
}
