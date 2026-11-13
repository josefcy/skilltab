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
    pub fn git(&self, dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }
    /// A jstack clone of a bare origin, with skills alpha and beta, a sync script and one declared source.
    /// The origin then moves one commit ahead (alpha's body changes) and the clone fetches it, so it is behind by 1.
    pub fn jstack_clone(&self) {
        let (seed, bare, clone) = (self.p("seed"), self.p("origin.git"), self.p("jstack"));
        fs::create_dir_all(&seed).unwrap();
        self.git(&seed, &["init", "-q", "-b", "main"]);
        for name in ["alpha", "beta"] {
            fs::create_dir_all(seed.join("skills").join(name)).unwrap();
            fs::write(seed.join("skills").join(name).join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nrepo body of {name}\n")).unwrap();
        }
        fs::write(seed.join("sync-skills.ps1"), "# sync\n").unwrap();
        fs::write(seed.join("skill-sources.txt"), "owner/pack one two\n").unwrap();
        self.git(&seed, &["add", "-A"]);
        self.git(&seed, &["commit", "-q", "-m", "init"]);
        self.git(&self.root, &["clone", "-q", "--bare", seed.to_str().unwrap(), bare.to_str().unwrap()]);
        self.git(&self.root, &["clone", "-q", bare.to_str().unwrap(), clone.to_str().unwrap()]);
        self.upstream_commit("upstream moves");
        self.git(&clone, &["fetch", "-q"]);
    }
    /// Another commit on the origin that the clone has not fetched.
    pub fn upstream_commit(&self, message: &str) {
        let seed = self.p("seed");
        fs::write(seed.join("skills").join("alpha").join("SKILL.md"), format!("---\nname: alpha\ndescription: d\n---\n{message}\n")).unwrap();
        self.git(&seed, &["commit", "-q", "-am", message]);
        self.git(&seed, &["push", "-q", self.p("origin.git").to_str().unwrap(), "main"]);
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

    let p = plan(&snap, &shelf, None, ManageAction::RemoveMount(claude.id.clone()));
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

    let p = plan(&snap, &shelf, None, ManageAction::RemoveMount(codex.id.clone()));
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

    let only_codex = plan(&snap, &shelf, None, ManageAction::RemoveMount(codex.id.clone()));
    assert_eq!(only_codex.blockers, vec![Blocker::SharedBody { dependents: vec![claude.id.clone()] }]);

    let p = plan(&snap, &shelf, None, ManageAction::DeleteBody(codex.body.clone()));
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

    let off = plan(&snap, &shelf, None, ManageAction::Disable(claude.id.clone()));
    results(apply(&off, &mut shelf, &FakeOs::default()));
    let snap = h.scan(&shelf, 2);
    assert_eq!(snap.mounts.len(), 1, "only Codex still sees it");
    assert_eq!(snap.shelf.len(), 1);
    assert_eq!(snap.shelf[0].reason, ShelfReason::Disabled);

    let on = plan(&snap, &shelf, None, ManageAction::Restore(snap.shelf[0].id.clone()));
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

    let off = plan(&snap, &shelf, None, ManageAction::Disable(m.id.clone()));
    assert!(matches!(off.steps.as_slice(), [Step::Stash { .. }]));
    results(apply(&off, &mut shelf, &FakeOs::default()));
    assert!(!h.p(".claude/skills/r").exists());
    let stash = shelf.stash_path(&shelf.entries()[0].id);
    assert!(stash.join("notes.txt").is_file(), "the folder sits in the shelf");

    let snap = h.scan(&shelf, 2);
    assert!(snap.mounts.is_empty());
    let on = plan(&snap, &shelf, None, ManageAction::Restore(snap.shelf[0].id.clone()));
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
        let p = plan(&snap, &shelf, None, action);
        assert_eq!(p.blockers, vec![Blocker::RecreatedBy { owner: Ownership::JstackSync, name: "x".into() }]);
        assert!(matches!(apply(&p, &mut shelf, &FakeOs::default()), ApplyOutcome::Refused));
    }
    let delete = plan(&snap, &shelf, None, ManageAction::DeleteBody(m.body.clone()));
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
        let p = plan(&snap, &shelf, None, action);
        assert!(p.blockers.contains(&Blocker::PartOfBundle { bundle: bundle.clone() }), "{:?}", p.blockers);
    }

    let already = plan(&snap, &shelf, None, ManageAction::Bundle(bundle.clone(), BundleOp::Enable));
    assert!(already.steps.is_empty() && already.blockers.is_empty());

    let off = plan(&snap, &shelf, None, ManageAction::Bundle(bundle.clone(), BundleOp::Disable));
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
    let p = plan(&snap, &shelf, None, ManageAction::RemoveMount(m.id.clone()));

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
    let p = plan(&snap, &shelf, None, ManageAction::DeleteBody(m.body.clone()));

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
        plan(&snap, &shelf, None, ManageAction::Disable(link.id.clone())),
        plan(&snap, &shelf, None, ManageAction::Disable(real.id.clone())),
    ] {
        assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Done]);
        assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Skipped]);
    }
    assert_eq!(shelf.entries().len(), 2);

    let snap = h.scan(&shelf, 2);
    for e in snap.shelf.clone() {
        let p = plan(&snap, &shelf, None, ManageAction::Restore(e.id.clone()));
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
    results(apply(&plan(&snap, &shelf, None, ManageAction::Disable(m.id.clone())), &mut shelf, &FakeOs::default()));

    let again = h.shelf();
    assert_eq!(again.entries().len(), 1);
    assert_eq!(again.entries()[0].name, "x");
    assert!(matches!(&again.entries()[0].held, Held::Link { target } if norm_key(target) == norm_key(&h.p(".agents/skills/x"))));

    fs::write(h.p(".skilltab-data/shelf.json"), "{not json").unwrap();
    assert!(Shelf::open(&h.p(".skilltab-data")).is_err());
}

fn jstack_plan(h: &Home, action: JstackAction) -> Plan {
    let repo = RepoState::load(&h.p("jstack")).unwrap();
    let shelf = h.shelf();
    plan(&h.scan(&shelf, 1), &shelf, Some(&repo), ManageAction::Jstack(action))
}

fn command(p: &Plan) -> &ExternalCommand {
    match p.steps.as_slice() {
        [Step::Run { command }] => command,
        other => panic!("expected one Run step, got {other:?}"),
    }
}

#[test]
fn jstack_plans_are_one_run_step_with_exactly_the_command_and_the_clone_as_cwd() {
    let h = Home::new();
    h.jstack_clone();
    let root = h.p("jstack");
    let text = |s: &str| s.to_owned();

    let sync = jstack_plan(&h, JstackAction::Sync);
    assert!(sync.blockers.is_empty(), "{:?}", sync.blockers);
    assert_eq!(
        *command(&sync),
        ExternalCommand {
            program: text("powershell"),
            args: vec![
                text("-NoProfile"),
                text("-ExecutionPolicy"),
                text("Bypass"),
                text("-File"),
                root.join("sync-skills.ps1").to_string_lossy().into_owned(),
                text("-repo"),
                root.to_string_lossy().into_owned(),
            ],
            cwd: Some(root.clone()),
        }
    );
    assert_eq!(sync.effects, vec![Effect::UpdatesClone, Effect::RelinksJstack]);

    let pull = jstack_plan(&h, JstackAction::Pull);
    assert_eq!(*command(&pull), ExternalCommand { program: text("git"), args: vec![text("pull"), text("--ff-only")], cwd: Some(root.clone()) });
    assert_eq!(pull.effects, vec![Effect::UpdatesClone]);

    let skills = vec![text("one"), text("two")];
    let install = jstack_plan(&h, JstackAction::InstallDeclared { source: text("owner/pack"), skills: skills.clone() });
    assert_eq!(
        *command(&install),
        ExternalCommand {
            program: text("npx.cmd"),
            args: ["--yes", "skills", "add", "owner/pack", "-g", "-a", "claude-code", "codex", "-y", "--skill", "one", "two"].map(text).into(),
            cwd: Some(root),
        }
    );
    assert_eq!(install.effects, vec![Effect::InstallsDeclared { source: text("owner/pack"), skills }], "it does not touch the clone");
}

#[test]
fn a_jstack_plan_with_no_clone_or_no_sync_script_is_unsupported_and_refused() {
    let h = Home::new();
    let shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    for action in [JstackAction::Sync, JstackAction::Pull, JstackAction::InstallDeclared { source: "s".into(), skills: vec!["a".into()] }] {
        let p = plan(&snap, &shelf, None, ManageAction::Jstack(action));
        assert!(p.steps.is_empty());
        assert!(matches!(p.blockers.as_slice(), [Blocker::Unsupported { .. }]), "{:?}", p.blockers);
        assert!(matches!(apply(&p, &mut h.shelf(), &FakeOs::default()), ApplyOutcome::Refused));
    }

    h.jstack_clone();
    fs::remove_file(h.p("jstack/sync-skills.ps1")).unwrap();
    let p = jstack_plan(&h, JstackAction::Sync);
    assert!(matches!(p.blockers.as_slice(), [Blocker::Unsupported { why, .. }] if why.contains("sync-skills.ps1")), "{:?}", p.blockers);
    assert!(jstack_plan(&h, JstackAction::Pull).blockers.is_empty(), "pull does not need the script");
    let none = jstack_plan(&h, JstackAction::InstallDeclared { source: "owner/pack".into(), skills: vec![] });
    assert!(matches!(none.blockers.as_slice(), [Blocker::Unsupported { .. }]));
}
