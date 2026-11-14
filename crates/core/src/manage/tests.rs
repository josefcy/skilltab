use super::*;
use crate::testutil::{unlink, Home};
use std::collections::BTreeSet;
use std::fs;
use std::sync::Mutex;

/// Records instead of recycling or running, and moves a "recycled" folder aside so the step's end state holds.
#[derive(Default)]
pub(crate) struct FakeOs {
    pub recycled: Mutex<Vec<PathBuf>>,
    pub ran: Mutex<Vec<String>>,
    /// The programs `find_tool` finds.
    pub tools: BTreeSet<String>,
}

impl FakeOs {
    pub(crate) fn with_tools(tools: &[&str]) -> FakeOs {
        FakeOs { tools: tools.iter().map(|t| (*t).to_owned()).collect(), ..FakeOs::default() }
    }
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
    fn find_tool(&self, program: &str) -> bool {
        self.tools.contains(program)
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
fn removing_a_linked_mount_leaves_its_target_and_can_be_restored() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    h.link(".claude/skills/x", ".agents/skills/x");
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

#[cfg(windows)]
#[test]
fn a_junction_made_by_mklink_resolves_as_a_link() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    let entry = h.p(".claude/skills/x");
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    let made = std::process::Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&entry).arg(h.p(".agents/skills/x")).output().unwrap();
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));

    assert!(still_link_to(&entry, &h.p(".agents/skills/x")));
    let snap = h.scan(&h.shelf(), 1);
    assert_eq!(mount(&snap, HarnessId::CLAUDE_CODE, &entry).hops.len(), 1);
}

#[cfg(unix)]
#[test]
fn a_symlink_resolves_as_a_link() {
    let h = Home::new();
    h.skill(".agents/skills/x", "x");
    let entry = h.p(".claude/skills/x");
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(h.p(".agents/skills/x"), &entry).unwrap();

    assert!(still_link_to(&entry, &h.p(".agents/skills/x")));
    let snap = h.scan(&h.shelf(), 1);
    assert_eq!(mount(&snap, HarnessId::CLAUDE_CODE, &entry).hops.len(), 1);
}

#[test]
fn a_rename_across_volumes_says_so_in_one_wording() {
    let e = io::Error::from(io::ErrorKind::CrossesDevices);
    assert!(moved_error(e, Path::new("x")).to_string().contains("another volume"));
}

#[test]
fn a_missing_program_is_reported_as_not_installed() {
    let command = ExternalCommand { program: "skilltab-no-such-tool".into(), args: vec![], cwd: None, env: vec![] };
    let e = SystemOs.run(&command).unwrap_err();
    assert!(e.to_string().contains("is not installed or not on PATH"), "{e}");
}

fn make_tool(dir: &Path, program: &str) {
    let file = dir.join(&sys::tool_names(program)[0]);
    fs::write(&file, "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn a_tool_is_found_by_the_name_this_platform_gives_an_executable() {
    let h = Home::new();
    make_tool(&h.root, "fake-tool");
    assert!(has_tool(&h.root, "fake-tool"));
    assert!(!has_tool(&h.root, "other-tool"));
    assert!(!has_tool(&h.p("missing"), "fake-tool"));
}

#[cfg(unix)]
#[test]
fn a_file_without_the_executable_bit_is_not_a_tool() {
    let h = Home::new();
    fs::write(h.p("fake-tool"), "").unwrap();
    assert!(!has_tool(&h.root, "fake-tool"));
}

#[cfg(windows)]
#[test]
fn a_cmd_shim_is_a_tool_on_windows() {
    let h = Home::new();
    fs::write(h.p("npx.cmd"), "").unwrap();
    assert!(has_tool(&h.root, "npx"));
}

#[test]
fn the_search_folders_are_path_first_and_then_the_ones_a_gui_launch_misses() {
    let h = Home::new();
    let path = std::env::join_paths([h.p("a"), h.p("b")]).unwrap();
    let dirs = tool_dirs(Some(path), Some(&h.p("home")));
    assert_eq!(dirs[..2], [h.p("a"), h.p("b")]);
    #[cfg(unix)]
    {
        let extra: Vec<PathBuf> = dirs[2..].to_vec();
        let wanted = [h.p("home/.local/bin"), PathBuf::from("/opt/homebrew/bin"), PathBuf::from("/usr/local/bin"), h.p("home/.npm-global/bin")];
        assert_eq!(extra, wanted);
    }
    #[cfg(windows)]
    assert_eq!(dirs.len(), 2);
    assert!(tool_dirs(None, None).iter().all(|d| d.is_absolute()));
}

#[test]
fn a_command_runs_with_the_environment_it_names() {
    let probe = vec![("SKILLTAB_PROBE".to_owned(), "from-the-command".to_owned())];
    #[cfg(windows)]
    let (program, args) = ("cmd", vec!["/C".to_owned(), "echo %SKILLTAB_PROBE%".to_owned()]);
    #[cfg(unix)]
    let (program, args) = ("sh", vec!["-c".to_owned(), "echo $SKILLTAB_PROBE".to_owned()]);
    let command = ExternalCommand { program: program.into(), args, cwd: None, env: probe };
    assert_eq!(SystemOs.run(&command).unwrap(), "from-the-command");
}

#[test]
fn removing_a_middle_hop_lists_every_mount_it_breaks() {
    let h = Home::new();
    h.skill("elsewhere/y", "y");
    h.link(".agents/skills/y", "elsewhere/y");
    h.link(".claude/skills/y", ".agents/skills/y");
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
    h.link(".claude/skills/x", ".agents/skills/x");
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
    h.link(".claude/skills/x", ".agents/skills/x");
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
    h.link(".claude/skills/x", ".agents/skills/x");
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let m = mount(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/x"));
    let p = plan(&snap, &shelf, ManageAction::RemoveMount(m.id.clone()));

    unlink(&h.p(".claude/skills/x"));
    h.link(".claude/skills/x", "other/x");

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
    h.link(".claude/skills/x", ".agents/skills/x");
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
    h.link(".claude/skills/x", ".agents/skills/x");
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

/// Real folders a, b and c. b is also reached by a Claude link. `keep` is not part of any selection.
fn bulk_home() -> Home {
    let h = Home::new();
    h.skill(".claude/skills/a", "a");
    h.skill(".agents/skills/b", "b");
    h.link(".claude/skills/b", ".agents/skills/b");
    h.skill(".claude/skills/c", "c");
    h.skill(".claude/skills/keep", "keep");
    h
}

fn body_of(snap: &Snapshot, harness: HarnessId, entry: &Path) -> SkillId {
    mount(snap, harness, entry).body.clone()
}

#[test]
fn a_bulk_delete_plans_every_selected_skill_as_one_change() {
    let h = bulk_home();
    let shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let ids = vec![
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/a")),
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/b")),
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/c")),
    ];

    let p = plan(&snap, &shelf, ManageAction::DeleteBodies(ids));
    assert!(p.blockers.is_empty() && p.skipped.is_empty(), "{:?}", p.blockers);
    let kinds: Vec<&str> = p.steps.iter().map(|s| if matches!(s, Step::RemoveLink { .. }) { "link" } else { "recycle" }).collect();
    assert_eq!(kinds, ["recycle", "link", "recycle", "recycle"], "{:?}", p.steps);
    assert!(p.effects.contains(&Effect::BodyRecycled { dir: h.p(".agents/skills/b") }));
}

#[test]
fn a_skill_that_cannot_be_deleted_is_skipped_and_the_rest_still_go() {
    let h = bulk_home();
    h.claude_plugin();
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let plugin_skill = snap.bodies.values().find(|b| b.manifest.name == "architect").unwrap();
    let Origin::Plugin { bundle } = &plugin_skill.origin else { panic!("{:?}", plugin_skill.origin) };
    let ids = vec![
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/a")),
        plugin_skill.id.clone(),
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/c")),
    ];

    let p = plan(&snap, &shelf, ManageAction::DeleteBodies(ids));
    assert!(p.blockers.is_empty(), "a skipped skill is not a blocker for the plan");
    assert_eq!(p.skipped.len(), 1);
    assert_eq!(p.skipped[0].skill, plugin_skill.id);
    assert!(p.skipped[0].blockers.contains(&Blocker::PartOfBundle { bundle: bundle.clone() }));
    assert_eq!(p.steps.len(), 2, "only a and c: {:?}", p.steps);

    let os = FakeOs::default();
    assert_eq!(results(apply(&p, &mut shelf, &os)), vec![StepResult::Done, StepResult::Done]);
    assert!(!h.p(".claude/skills/a").exists() && !h.p(".claude/skills/c").exists());
    assert!(plugin_skill.dir.as_path().is_dir(), "the plugin's folder is untouched");
    assert!(h.p(".claude/skills/keep").is_dir());
}

#[test]
fn an_id_that_vanished_is_skipped_as_no_longer_installed() {
    let h = bulk_home();
    let shelf = h.shelf();
    let before = h.scan(&shelf, 1);
    let gone = body_of(&before, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/c"));
    let a = body_of(&before, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/a"));
    fs::remove_dir_all(h.p(".claude/skills/c")).unwrap();
    let snap = h.scan(&shelf, 2);

    let p = plan(&snap, &shelf, ManageAction::DeleteBodies(vec![gone.clone(), a]));
    assert!(p.blockers.is_empty());
    assert_eq!(p.skipped.len(), 1);
    assert_eq!(p.skipped[0].skill, gone);
    assert!(matches!(p.skipped[0].blockers.as_slice(), [Blocker::Unsupported { why, .. }] if why.contains("no longer installed")));
    assert!(matches!(p.steps.as_slice(), [Step::Recycle { .. }]));
}

#[test]
fn an_empty_or_fully_skipped_selection_has_nothing_to_apply() {
    let h = Home::new();
    h.claude_plugin();
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let plugin_skill = snap.bodies.keys().next().unwrap().clone();

    let empty = plan(&snap, &shelf, ManageAction::DeleteBodies(Vec::new()));
    assert!(empty.steps.is_empty() && empty.skipped.is_empty() && empty.blockers.is_empty());

    let all_skipped = plan(&snap, &shelf, ManageAction::DeleteBodies(vec![plugin_skill]));
    assert!(all_skipped.steps.is_empty() && all_skipped.blockers.is_empty());
    assert_eq!(all_skipped.skipped.len(), 1);
    let os = FakeOs::default();
    assert_eq!(results(apply(&all_skipped, &mut shelf, &os)), Vec::<StepResult>::new());
    assert!(os.recycled.lock().unwrap().is_empty());
}

#[test]
fn applying_a_bulk_delete_recycles_real_folders_only_and_leaves_what_it_did_not_select() {
    let h = bulk_home();
    let mut shelf = h.shelf();
    let snap = h.scan(&shelf, 1);
    let ids = vec![
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/b")),
        body_of(&snap, HarnessId::CLAUDE_CODE, &h.p(".claude/skills/c")),
    ];
    let p = plan(&snap, &shelf, ManageAction::DeleteBodies(ids));

    let os = FakeOs::default();
    assert!(results(apply(&p, &mut shelf, &os)).iter().all(|r| *r == StepResult::Done));

    let mut recycled = os.recycled.lock().unwrap().clone();
    recycled.sort();
    assert_eq!(recycled, vec![h.p(".agents/skills/b"), h.p(".claude/skills/c")], "the link at .claude/skills/b is unlinked, never recycled");
    assert!(!h.p(".claude/skills/b").exists(), "the link is gone");
    assert!(h.p(".claude/skills/a").is_dir() && h.p(".claude/skills/keep").is_dir());
    assert!(shelf.entries().is_empty(), "a deleted skill is not shelved");
}

fn plan_of(steps: Vec<Step>) -> Plan {
    let mut p = Plan::new(1, ManageAction::DeleteBodies(Vec::new()));
    p.steps = steps;
    p
}

fn write(to: &Path, bytes: &[u8], was: Was) -> Step {
    Step::Write { to: to.to_path_buf(), bytes: Arc::from(bytes), was }
}

fn hash_of(bytes: &[u8]) -> Was {
    Was::Hash(ContentHash::of(bytes))
}

#[test]
fn a_new_file_is_written_with_its_parents_and_a_second_apply_skips_it() {
    let h = Home::new();
    let mut shelf = h.shelf();
    let to = h.p("out/deep/a.txt");
    let p = plan_of(vec![write(&to, b"one", Was::Absent)]);

    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Done]);
    assert_eq!(fs::read(&to).unwrap(), b"one");
    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Skipped]);
}

#[test]
fn a_write_over_absent_never_replaces_a_file_that_appeared_after_planning() {
    let h = Home::new();
    let mut shelf = h.shelf();
    let to = h.p("out/a.txt");
    let p = plan_of(vec![write(&to, b"mine", Was::Absent)]);
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::write(&to, b"theirs").unwrap();

    let ApplyOutcome::Stale { reasons } = apply(&p, &mut shelf, &FakeOs::default()) else { panic!("expected Stale") };
    assert!(reasons[0].contains("a.txt"), "{reasons:?}");
    assert_eq!(fs::read(&to).unwrap(), b"theirs");
}

#[test]
fn a_create_that_loses_the_race_fails_instead_of_overwriting() {
    let h = Home::new();
    let to = h.p("a.txt");
    fs::write(&to, b"theirs").unwrap();

    let e = write_file(&to, b"mine", &Was::Absent).unwrap_err();
    assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read(&to).unwrap(), b"theirs");
}

#[test]
fn a_replacement_goes_through_a_temp_file_and_leaves_none_behind() {
    let h = Home::new();
    let mut shelf = h.shelf();
    let to = h.p("out/a.txt");
    fs::create_dir_all(to.parent().unwrap()).unwrap();
    fs::write(&to, b"old").unwrap();
    let p = plan_of(vec![write(&to, b"new", hash_of(b"old"))]);

    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Done]);
    assert_eq!(fs::read(&to).unwrap(), b"new");
    let left: Vec<_> = fs::read_dir(to.parent().unwrap()).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(left, ["a.txt"]);
}

#[test]
fn a_replacement_of_a_file_that_changed_since_planning_is_refused_and_nothing_is_written() {
    let h = Home::new();
    let mut shelf = h.shelf();
    let (a, b) = (h.p("out/a.txt"), h.p("out/b.txt"));
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::write(&a, b"old").unwrap();
    let p = plan_of(vec![write(&b, b"fresh", Was::Absent), write(&a, b"new", hash_of(b"old"))]);
    fs::write(&a, b"edited meanwhile").unwrap();

    let ApplyOutcome::Stale { reasons } = apply(&p, &mut shelf, &FakeOs::default()) else { panic!("expected Stale") };
    assert!(reasons[0].contains("a.txt"), "{reasons:?}");
    assert_eq!(fs::read(&a).unwrap(), b"edited meanwhile");
    assert!(!b.exists(), "a refused plan runs no step");
}

#[test]
fn a_delete_removes_only_a_file_that_still_has_the_planned_content() {
    let h = Home::new();
    let mut shelf = h.shelf();
    let path = h.p("out/a.txt");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"old").unwrap();
    let step = Step::Delete { path: path.clone(), was: ContentHash::of(b"old") };

    fs::write(&path, b"edited").unwrap();
    assert!(matches!(apply(&plan_of(vec![step.clone()]), &mut shelf, &FakeOs::default()), ApplyOutcome::Stale { .. }));
    assert!(path.exists());

    fs::write(&path, b"old").unwrap();
    let p = plan_of(vec![step]);
    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Done]);
    assert!(!path.exists());
    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Skipped]);
}

#[test]
fn a_first_link_creates_its_parents_and_a_second_apply_skips_it() {
    let h = Home::new();
    h.skill("repo/skills/own", "own");
    let mut shelf = h.shelf();
    let (entry, target) = (h.p(".claude/skills/own"), h.p("repo/skills/own"));
    let p = plan_of(vec![Step::Link { entry: entry.clone(), target: target.clone(), shelf: None }]);

    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Done]);
    assert!(still_link_to(&entry, &target));
    assert_eq!(results(apply(&p, &mut shelf, &FakeOs::default())), vec![StepResult::Skipped]);

    fs::remove_dir_all(&target).unwrap();
    let again = plan_of(vec![Step::Link { entry: h.p(".agents/skills/own"), target, shelf: None }]);
    assert!(matches!(apply(&again, &mut shelf, &FakeOs::default()), ApplyOutcome::Stale { .. }), "a link to nothing is refused");
}
