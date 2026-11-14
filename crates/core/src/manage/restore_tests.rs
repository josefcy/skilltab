//! Restore end to end through the facade, with a fake OS that acts like the real tools. The scenarios are those of
//! jstack's `test-restore.ps1`, plus what a cloned repo must never be able to do.

use super::tests::FakeOs;
use super::*;
use crate::scan::still_link_to;
use crate::setup::{read_restore, SetupDir};
use crate::testutil::{listing, Home};
use crate::view::{self, ActionRequest, ApplyOutcomeKind, NoteKind as ViewNote, PlanView, StepKind, StepResultView};
use crate::{CoreError, Skilltab};
use std::fs;

const FULL: &str = r#"{
  "marketplaces": [
    { "name": "alpha", "source": { "source": "github", "repo": "acme/alpha" } },
    { "name": "beta", "source": { "source": "git", "url": "https://example.com/acme/beta.git" } }
  ],
  "claudePlugins": ["plug@alpha"],
  "skillSources": [
    { "source": "acme/skills", "skills": ["s1", "s2", "s3"] },
    { "source": "https://example.test", "skills": ["s4"] }
  ],
  "ownSkills": ["own1", "own2"]
}"#;

const COMMANDS: [&str; 5] = [
    "claude plugin marketplace add acme/alpha",
    "claude plugin marketplace add https://example.com/acme/beta.git",
    "claude plugin install plug@alpha",
    "npx --yes skills add acme/skills -g -a claude-code codex -y --skill s1 s2 s3",
    "npx --yes skills add https://example.test -g -a claude-code codex -y --skill s4",
];

/// A setup folder at `<home>/repo` with the two own skills and three config files.
fn setup_folder(h: &Home, manifest: &str) {
    fs::create_dir_all(h.p("repo/skills/own1")).unwrap();
    fs::create_dir_all(h.p("repo/skills/own2")).unwrap();
    fs::write(h.p("repo/skills/own1/SKILL.md"), "own1").unwrap();
    fs::write(h.p("repo/skills/own2/SKILL.md"), "own2").unwrap();
    fs::write(h.p("repo/manifest.json"), manifest).unwrap();
    write(h, "repo/config/claude/CLAUDE.md", "from the backup");
    write(h, "repo/config/claude/rules/r.md", "rule");
    write(h, "repo/config/codex/AGENTS.md", "agents");
}

fn write(h: &Home, rel: &str, text: &str) {
    fs::create_dir_all(h.p(rel).parent().unwrap()).unwrap();
    fs::write(h.p(rel), text).unwrap();
}

struct Rig {
    h: Home,
    app: Skilltab,
    os: Arc<FakeOs>,
}

impl Rig {
    fn new(h: Home, os: FakeOs) -> Rig {
        let os = Arc::new(os);
        let app = Skilltab::open_with(h.env(), h.p(".skilltab-data"), Box::new(os.clone())).unwrap();
        Rig { h, app, os }
    }

    fn with_tools(h: Home) -> Rig {
        Rig::new(h, FakeOs::with_tools(&["claude", "npx"]))
    }

    fn request(&self) -> ActionRequest {
        ActionRequest::RestoreSetup { folder: self.h.p("repo").to_string_lossy().into_owned() }
    }

    fn plan(&self) -> PlanView {
        self.app.plan(&self.request()).unwrap()
    }

    fn plan_err(&self) -> CoreError {
        self.app.plan(&self.request()).err().expect("the request is refused")
    }

    fn apply(&self, plan: &PlanView) -> view::ApplyReport {
        self.app.apply(&plan.id).unwrap()
    }

    fn ran(&self) -> Vec<String> {
        self.os.ran.lock().unwrap().clone()
    }
}

fn slashes(text: &str) -> String {
    text.replace('\\', "/")
}

fn steps(plan: &PlanView) -> Vec<(StepKind, String)> {
    plan.steps.iter().map(|s| (s.kind, slashes(&s.detail))).collect()
}

fn notes(plan: &PlanView, kind: ViewNote) -> Vec<String> {
    plan.notes.iter().filter(|n| n.kind == kind).map(|n| slashes(&n.text)).collect()
}

fn path(h: &Home, rel: &str) -> String {
    slashes(&view::path_str(&h.p(rel)))
}

fn link_detail(h: &Home, entry: &str, target: &str) -> String {
    format!("{} → {}", path(h, entry), path(h, target))
}

fn invalid(e: CoreError) -> String {
    match e {
        CoreError::Invalid { message } => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn an_empty_home_plans_links_and_files_first_then_the_exact_commands_and_a_second_plan_has_nothing_to_do() {
    let h = Home::new();
    setup_folder(&h, FULL);
    let r = Rig::with_tools(h);

    let plan = r.plan();
    assert_eq!(plan.title, format!("Restore setup from {}", view::path_str(&r.h.p("repo"))));
    assert_eq!(plan.confirm, "Restore setup");
    assert!(plan.ready && plan.blockers.is_empty());
    let h = &r.h;
    let mut expected = vec![
        (StepKind::Link, link_detail(h, ".claude/skills/own1", "repo/skills/own1")),
        (StepKind::Link, link_detail(h, ".agents/skills/own1", "repo/skills/own1")),
        (StepKind::Link, link_detail(h, ".claude/skills/own2", "repo/skills/own2")),
        (StepKind::Link, link_detail(h, ".agents/skills/own2", "repo/skills/own2")),
        (StepKind::Write, path(h, ".claude/CLAUDE.md")),
        (StepKind::Write, path(h, ".claude/rules/r.md")),
        (StepKind::Write, path(h, ".codex/AGENTS.md")),
    ];
    expected.extend(COMMANDS.iter().map(|c| (StepKind::Run, (*c).to_owned())));
    assert_eq!(steps(&plan), expected);
    assert_eq!(
        notes(&plan, ViewNote::Manual),
        ["Sign in to Claude Code. Run claude and follow the login prompt.", "Sign in to Codex. Run codex login."]
    );

    let report = r.apply(&plan);
    assert_eq!(report.outcome, ApplyOutcomeKind::Applied);
    assert!(report.steps.iter().all(|s| s.result == StepResultView::Done), "{:?}", report.steps.iter().map(|s| (&s.detail, &s.message)).collect::<Vec<_>>());
    assert_eq!(r.ran(), COMMANDS);
    for entry in [".claude/skills/own1", ".agents/skills/own1", ".claude/skills/own2", ".agents/skills/own2"] {
        let name = entry.rsplit('/').next().unwrap();
        assert!(still_link_to(&h.p(entry), &h.p(&format!("repo/skills/{name}"))), "{entry}");
    }
    assert_eq!(fs::read_to_string(h.p(".claude/CLAUDE.md")).unwrap(), "from the backup");
    assert_eq!(fs::read_to_string(h.p(".claude/rules/r.md")).unwrap(), "rule");
    assert_eq!(fs::read_to_string(h.p(".codex/AGENTS.md")).unwrap(), "agents");

    let again = r.plan();
    assert!(!again.ready && again.steps.is_empty() && again.blockers.is_empty(), "{:?}", steps(&again));
    assert_eq!(notes(&again, ViewNote::Present).len(), 4 + 3 + 3 + 2, "{:?}", notes(&again, ViewNote::Present));
    assert!(notes(&again, ViewNote::Attention).is_empty());
    assert_eq!(notes(&again, ViewNote::Manual).len(), 2, "signing in is still the user's");
    assert_eq!(r.apply(&again).steps.len(), 0);
    assert_eq!(r.ran(), COMMANDS, "nothing ran twice");
}

#[test]
fn a_half_set_up_home_plans_only_what_is_missing_and_notes_the_rest() {
    let h = Home::new();
    setup_folder(&h, FULL);
    write(&h, ".claude/plugins/known_marketplaces.json", r#"{ "alpha": { "source": { "source": "github", "repo": "acme/alpha" } } }"#);
    fs::create_dir_all(h.p(".agents/skills/s1")).unwrap();
    fs::create_dir_all(h.p(".claude/skills/s1")).unwrap();
    write(&h, ".agents/skills/own2/marker.txt", "keep me");
    write(&h, ".claude/CLAUDE.md", "my own");
    let r = Rig::with_tools(h);
    let h = &r.h;

    let plan = r.plan();
    assert_eq!(
        steps(&plan),
        [
            (StepKind::Link, link_detail(h, ".claude/skills/own1", "repo/skills/own1")),
            (StepKind::Link, link_detail(h, ".agents/skills/own1", "repo/skills/own1")),
            (StepKind::Link, link_detail(h, ".claude/skills/own2", "repo/skills/own2")),
            (StepKind::Write, path(h, ".claude/CLAUDE.md.from-backup")),
            (StepKind::Write, path(h, ".claude/rules/r.md")),
            (StepKind::Write, path(h, ".codex/AGENTS.md")),
            (StepKind::Run, "claude plugin marketplace add https://example.com/acme/beta.git".to_owned()),
            (StepKind::Run, "claude plugin install plug@alpha".to_owned()),
            (StepKind::Run, "npx --yes skills add acme/skills -g -a claude-code codex -y --skill s2 s3".to_owned()),
            (StepKind::Run, "npx --yes skills add https://example.test -g -a claude-code codex -y --skill s4".to_owned()),
        ]
    );
    let present = notes(&plan, ViewNote::Present);
    assert!(present.iter().any(|n| n.contains("marketplace alpha")), "{present:?}");
    assert!(present.iter().any(|n| n.contains("acme/skills") && n.ends_with(": s1.")), "{present:?}");
    let attention = notes(&plan, ViewNote::Attention);
    assert_eq!(attention.len(), 2, "{attention:?}");
    assert!(attention.iter().any(|n| n.contains(".agents/skills/own2") && n.contains("Left alone")), "{attention:?}");
    assert!(attention.iter().any(|n| n.contains("CLAUDE.md differs") && n.contains("CLAUDE.md.from-backup")), "{attention:?}");

    assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Applied);
    assert_eq!(fs::read_to_string(h.p(".claude/CLAUDE.md")).unwrap(), "my own", "a differing file is not overwritten");
    assert_eq!(fs::read_to_string(h.p(".claude/CLAUDE.md.from-backup")).unwrap(), "from the backup");
    assert_eq!(fs::read_to_string(h.p(".agents/skills/own2/marker.txt")).unwrap(), "keep me");
    assert!(still_link_to(&h.p(".claude/skills/own2"), &h.p("repo/skills/own2")), "the other harness folder still gets its link");
    assert_eq!(r.ran().len(), 4);

    let again = r.plan();
    assert!(again.steps.is_empty(), "{:?}", steps(&again));
    assert_eq!(notes(&again, ViewNote::Attention).len(), 2, "the two things left alone stay named");
    assert_eq!(fs::read_to_string(h.p(".agents/skills/own2/marker.txt")).unwrap(), "keep me");
}

#[test]
fn a_differing_config_file_gets_a_from_backup_copy_and_is_never_written_over() {
    let h = Home::new();
    setup_folder(&h, "{}");
    write(&h, ".claude/CLAUDE.md", "mine");
    write(&h, ".codex/AGENTS.md", "mine too");
    write(&h, ".codex/AGENTS.md.from-backup", "an older backup");
    write(&h, ".claude/rules/r.md", "rule");
    fs::write(h.p("repo/config/claude/rules/r.md"), "rule\r\n").unwrap();
    fs::write(h.p(".claude/rules/r.md"), "rule\n").unwrap();
    let r = Rig::with_tools(h);
    let h = &r.h;

    let plan = r.plan();
    assert_eq!(
        steps(&plan),
        [(StepKind::Write, path(h, ".claude/CLAUDE.md.from-backup")), (StepKind::Write, path(h, ".codex/AGENTS.md.from-backup"))]
    );
    assert_eq!((plan.steps[0].text.as_str(), plan.steps[1].text.as_str()), ("Write a new file", "Replace the file"));
    assert!(notes(&plan, ViewNote::Present).iter().any(|n| n.contains("rules/r.md")), "a CRLF checkout is not a difference");
    assert_eq!(notes(&plan, ViewNote::Attention).len(), 2);

    assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Applied);
    assert_eq!(fs::read_to_string(h.p(".claude/CLAUDE.md")).unwrap(), "mine");
    assert_eq!(fs::read_to_string(h.p(".codex/AGENTS.md")).unwrap(), "mine too");
    assert_eq!(fs::read_to_string(h.p(".claude/CLAUDE.md.from-backup")).unwrap(), "from the backup");
    assert_eq!(fs::read_to_string(h.p(".codex/AGENTS.md.from-backup")).unwrap(), "agents");

    let again = r.plan();
    assert!(again.steps.is_empty(), "{:?}", steps(&again));
    let attention = notes(&again, ViewNote::Attention);
    assert_eq!(attention.len(), 2);
    assert!(attention.iter().all(|n| n.contains("already at")), "{attention:?}");
}

#[test]
fn a_file_that_appears_after_planning_is_not_written_over() {
    let h = Home::new();
    setup_folder(&h, "{}");
    let r = Rig::with_tools(h);
    let plan = r.plan();
    write(&r.h, ".claude/CLAUDE.md", "appeared meanwhile");

    let report = r.apply(&plan);
    assert_eq!(report.outcome, ApplyOutcomeKind::Stale);
    assert!(report.reasons.iter().any(|m| m.contains("CLAUDE.md")), "{:?}", report.reasons);
    assert_eq!(fs::read_to_string(r.h.p(".claude/CLAUDE.md")).unwrap(), "appeared meanwhile");
    assert!(!r.h.p(".codex/AGENTS.md").exists(), "nothing ran");
}

#[test]
fn a_real_folder_or_a_link_to_another_target_at_an_own_skill_path_is_left_alone() {
    let h = Home::new();
    setup_folder(&h, r#"{"ownSkills":["own1","own2"]}"#);
    write(&h, ".claude/skills/own1/marker.txt", "keep me");
    write(&h, "elsewhere/x/SKILL.md", "x");
    h.link(".agents/skills/own1", "elsewhere/x");
    h.link(".claude/skills/own2", "repo/skills/own2");
    let r = Rig::with_tools(h);
    let h = &r.h;

    let plan = r.plan();
    assert_eq!(steps(&plan), [(StepKind::Link, link_detail(h, ".agents/skills/own2", "repo/skills/own2"))].into_iter().chain([
        (StepKind::Write, path(h, ".claude/CLAUDE.md")),
        (StepKind::Write, path(h, ".claude/rules/r.md")),
        (StepKind::Write, path(h, ".codex/AGENTS.md")),
    ]).collect::<Vec<_>>());
    let attention = notes(&plan, ViewNote::Attention);
    assert_eq!(attention.len(), 2, "{attention:?}");
    assert!(attention.iter().any(|n| n.contains(".claude/skills/own1") && n.contains("not a link to")), "{attention:?}");
    assert!(attention.iter().any(|n| n.contains(".agents/skills/own1") && n.contains("Left alone")), "{attention:?}");
    assert!(notes(&plan, ViewNote::Present).iter().any(|n| n.contains(".claude/skills/own2")));

    assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Applied);
    assert_eq!(fs::read_to_string(h.p(".claude/skills/own1/marker.txt")).unwrap(), "keep me");
    assert!(!fs::symlink_metadata(h.p(".claude/skills/own1")).unwrap().file_type().is_symlink());
    assert!(still_link_to(&h.p(".agents/skills/own1"), &h.p("elsewhere/x")));
}

#[test]
fn an_entry_that_appears_at_a_planned_link_after_planning_is_not_replaced() {
    let h = Home::new();
    setup_folder(&h, r#"{"ownSkills":["own1"]}"#);
    let r = Rig::with_tools(h);
    let plan = r.plan();
    write(&r.h, ".agents/skills/own1/marker.txt", "keep me");

    assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Stale);
    assert_eq!(fs::read_to_string(r.h.p(".agents/skills/own1/marker.txt")).unwrap(), "keep me");
}

#[test]
fn an_own_skill_with_no_folder_in_the_repo_is_named_and_the_others_still_link() {
    let h = Home::new();
    setup_folder(&h, r#"{"ownSkills":["gone","own1"]}"#);
    let r = Rig::with_tools(h);

    let plan = r.plan();
    assert_eq!(plan.steps.iter().filter(|s| s.kind == StepKind::Link).count(), 2);
    let attention = notes(&plan, ViewNote::Attention);
    assert_eq!(attention.len(), 1, "named once, not once per harness folder");
    assert!(attention[0].contains("gone") && attention[0].contains("skills/"), "{attention:?}");
    assert!(plan.ready);
}

#[test]
fn a_cloned_repo_cannot_write_outside_the_allowlist() {
    let h = Home::new();
    setup_folder(&h, "{}");
    write(&h, "repo/config/ssh/id_rsa", "key");
    write(&h, "repo/config/claude/settings.json", "{}");
    write(&h, "repo/config/claude/rules/notes.txt", "x");
    let r = Rig::with_tools(h);

    let plan = r.plan();
    let attention = notes(&plan, ViewNote::Attention);
    assert_eq!(attention.len(), 3, "{attention:?}");
    for name in ["config/claude/rules/notes.txt", "config/claude/settings.json", "config/ssh/id_rsa"] {
        assert!(attention.iter().any(|n| n.starts_with(name) && n.contains("never written")), "{name}: {attention:?}");
    }
    assert!(plan.steps.iter().all(|s| !s.detail.contains("ssh") && !s.detail.contains("settings") && !s.detail.contains("notes.txt")));

    assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Applied);
    assert!(!r.h.p(".ssh").exists() && !r.h.p(".claude/settings.json").exists() && !r.h.p(".claude/rules/notes.txt").exists());
}

#[test]
fn a_flood_of_ignored_files_is_named_in_part_and_counted() {
    let h = Home::new();
    setup_folder(&h, "{}");
    for n in 0..25 {
        write(&h, &format!("repo/config/junk/f{n:02}"), "x");
    }
    let plan = Rig::with_tools(h).plan();
    let attention = notes(&plan, ViewNote::Attention);
    assert_eq!(attention.len(), 21);
    assert!(attention[20].starts_with("5 more files"), "{attention:?}");
}

#[test]
fn a_folder_that_is_not_a_setup_folder_or_holds_a_hostile_manifest_fails_with_a_message_and_runs_nothing() {
    let h = Home::new();
    fs::create_dir_all(h.p("repo")).unwrap();
    let r = Rig::with_tools(h);
    assert!(invalid(r.plan_err()).contains("does not look like a setup folder"));

    let hostile = [
        r#"{"claudePlugins":["--scope@x"]}"#,
        r#"{"skillSources":[{"source":"-y","skills":["a"]}]}"#,
        r#"{"skillSources":[{"source":"acme/skills","skills":["--yes"]}]}"#,
        r#"{"ownSkills":[".."]}"#,
        r#"{"ownSkills":["../up"]}"#,
        r#"{"ownSkills":["a/b"]}"#,
        r#"{"ownSkills":["a\\b"]}"#,
        r#"{"marketplaces":[{"name":"m","source":{"source":"git","url":"-oProxyCommand=x"}}]}"#,
        "not json",
    ];
    for text in hostile {
        fs::write(r.h.p("repo/manifest.json"), text).unwrap();
        let why = invalid(r.plan_err());
        assert!(why.contains("does not look like a setup folder") && why.contains("manifest.json"), "{text}: {why}");
    }
    assert!(r.ran().is_empty());

    let err = r.app.plan(&ActionRequest::RestoreSetup { folder: "repo".into() });
    assert!(matches!(err, Err(CoreError::Invalid { .. })), "a folder that is not a full path is refused");
}

#[test]
fn a_missing_tool_blocks_with_a_remedy_and_apply_refuses_and_changes_nothing() {
    for (tools, missing) in [(&[][..], vec!["claude", "npx"]), (&["npx"][..], vec!["claude"]), (&["claude"][..], vec!["npx"])] {
        let h = Home::new();
        setup_folder(&h, FULL);
        let r = Rig::new(h, FakeOs::with_tools(tools));
        let before = listing(&r.h.root);

        let plan = r.plan();
        assert!(!plan.ready);
        assert_eq!(plan.blockers.len(), missing.len(), "{:?}", plan.blockers.iter().map(|b| &b.reason).collect::<Vec<_>>());
        for tool in &missing {
            let blocker = plan.blockers.iter().find(|b| b.reason.contains(&format!("`{tool}`"))).unwrap_or_else(|| panic!("no blocker for {tool}"));
            assert!(blocker.remedy.contains("plan the restore again"), "{}", blocker.remedy);
        }
        let remedies: String = plan.blockers.iter().map(|b| b.remedy.as_str()).collect();
        assert_eq!(remedies.contains("claude.com/claude-code"), missing.contains(&"claude"));
        assert_eq!(remedies.contains("nodejs.org"), missing.contains(&"npx"));

        assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Refused);
        assert!(r.ran().is_empty());
        assert_eq!(listing(&r.h.root), before, "not even the links and files went in");
    }
}

#[test]
fn only_the_programs_the_plan_runs_are_checked() {
    let h = Home::new();
    setup_folder(&h, r#"{"ownSkills":["own1"]}"#);
    let plan = Rig::new(h, FakeOs::default()).plan();
    assert!(plan.blockers.is_empty() && plan.ready, "no command to run, so no tool is needed");

    let h = Home::new();
    setup_folder(&h, r#"{"claudePlugins":["p@m"]}"#);
    let plan = Rig::new(h, FakeOs::with_tools(&["claude"])).plan();
    assert!(plan.blockers.is_empty(), "npx is not needed when there is no skill source");
}

#[test]
fn planning_is_the_dry_run_and_changes_nothing_on_disk() {
    let h = Home::new();
    setup_folder(&h, FULL);
    write(&h, ".claude/CLAUDE.md", "my own");
    fs::create_dir_all(h.p(".agents/skills/own2")).unwrap();
    let r = Rig::with_tools(h);
    let before = listing(&r.h.root);

    let plan = r.plan();
    assert!(plan.ready && plan.steps.len() > 5);
    assert_eq!(listing(&r.h.root), before);
    assert!(r.ran().is_empty());
}

#[test]
fn a_failing_command_stops_the_rest_and_the_next_plan_has_only_what_is_left() {
    let h = Home::new();
    setup_folder(&h, FULL);
    let r = Rig::new(h, FakeOs::with_tools(&["claude", "npx"]).failing_on("claude plugin install"));

    let plan = r.plan();
    assert_eq!(plan.steps.len(), 12);
    let report = r.apply(&plan);
    assert_eq!(report.outcome, ApplyOutcomeKind::Failed);
    let results: Vec<StepResultView> = report.steps.iter().map(|s| s.result).collect();
    let mut expected = vec![StepResultView::Done; 9];
    expected.push(StepResultView::Failed);
    expected.extend([StepResultView::NotRun; 2]);
    assert_eq!(results, expected);
    assert!(report.steps[9].message.as_deref().unwrap().contains("boom"));
    assert_eq!(r.ran(), COMMANDS[..3], "nothing after the failure ran");
    assert!(still_link_to(&r.h.p(".claude/skills/own1"), &r.h.p("repo/skills/own1")), "the local steps were not stranded");
    assert_eq!(fs::read_to_string(r.h.p(".claude/CLAUDE.md")).unwrap(), "from the backup");

    let again = r.plan();
    assert_eq!(
        steps(&again),
        [
            (StepKind::Run, COMMANDS[2].to_owned()),
            (StepKind::Run, COMMANDS[3].to_owned()),
            (StepKind::Run, COMMANDS[4].to_owned()),
        ],
        "the marketplaces are in, and the links and files are not planned again"
    );
}

#[test]
fn an_unreadable_plugin_record_blocks_the_restore() {
    let h = Home::new();
    setup_folder(&h, FULL);
    write(&h, ".claude/plugins/known_marketplaces.json", "{oops");
    let r = Rig::with_tools(h);

    let plan = r.plan();
    assert!(!plan.ready && plan.steps.is_empty());
    assert!(plan.blockers.iter().any(|b| b.reason.contains("known_marketplaces.json") && b.remedy.contains("plan the restore again")));
    assert_eq!(r.apply(&plan).outcome, ApplyOutcomeKind::Refused);
}

#[test]
fn every_command_runs_in_the_restores_home_and_through_a_vec_of_args_not_a_shell() {
    let h = Home::new();
    setup_folder(&h, FULL);
    let (env, os) = (h.env(), FakeOs::with_tools(&["claude", "npx"]));
    let input = read_restore(&env, &os, SetupDir::parse(&env, "~/repo").unwrap()).unwrap();
    let shelf = h.shelf();
    let snap = h.scan(&shelf, 1);

    let p = plan(&snap, &shelf, ManageAction::RestoreSetup(Arc::new(input)));
    let commands: Vec<&ExternalCommand> = p.steps.iter().filter_map(|s| if let Step::Run { command } = s { Some(command) } else { None }).collect();
    assert_eq!(commands.len(), 5);
    let home = h.root.to_string_lossy().into_owned();
    for command in commands {
        assert_eq!(command.env, [("HOME".to_owned(), home.clone()), ("USERPROFILE".to_owned(), home.clone())], "{}", command.display());
    }
}
