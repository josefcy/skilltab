//! Policy for the setup plans. Pure: it turns the input the `setup` module read into steps, notes and blockers.

use super::{Blocker, Note, NoteKind, Plan, Step, Was};
use crate::harness::{registry, ExternalCommand, PluginRecord, PluginState};
use crate::model::{norm_key, ContentHash};
use crate::setup::{find_secret, relative_label, Entry, ExportInput, RestoreInput, Wanted, SKILLS_CLI};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many ignored files a plan names one by one before it sums up the rest.
const IGNORED_LISTED: usize = 20;

impl Plan {
    fn note(&mut self, kind: NoteKind, text: String) {
        self.notes.push(Note { kind, text });
    }
}

/// Writes `manifest.json` and the allowlisted config files into the setup folder, and deletes the copies of
/// files that left the machine. Anything that is already as wanted becomes a Present note with no step.
/// An unreadable input or a secret blocks the whole export, so a half-read machine never reaches the folder.
pub(super) fn plan_export(i: &ExportInput, p: &mut Plan) {
    for (name, kind) in &i.unrebuildable {
        p.note(
            NoteKind::Attention,
            format!("Marketplace {name} has a source of type \"{kind}\", which restore cannot rebuild. It is left out of manifest.json."),
        );
    }
    for name in &i.unrestorable_skills {
        p.note(
            NoteKind::Attention,
            format!("Skill {name} is in the skill lock with no source and no url, so a restore cannot install it. It is left out of manifest.json."),
        );
    }
    for problem in &i.problems {
        p.block(Blocker::Unsupported { why: problem.why.clone(), remedy: problem.remedy.clone() });
    }
    for file in &i.sources {
        if let Some(pattern) = find_secret(&file.bytes) {
            p.block(Blocker::Unsupported {
                why: format!("{} looks like it holds a secret. It matches the pattern {pattern}.", file.label()),
                remedy: "Remove the secret from that file, then plan the export again.".to_owned(),
            });
        }
    }
    if !p.blockers.is_empty() {
        return;
    }

    let manifest: Arc<[u8]> = i.manifest.render().into_bytes().into();
    put(p, i.dir.manifest_path(), manifest, i.on_disk_manifest.as_deref(), "manifest.json");
    for file in &i.sources {
        let existing = i.in_repo.iter().find(|r| r.same_file(file));
        let to = existing.map_or_else(|| file.repo_path(&i.dir), |r| r.repo_path(&i.dir));
        put(p, to, file.bytes.clone(), existing.map(|r| &*r.bytes), &file.label());
    }
    for gone in i.in_repo.iter().filter(|r| !i.sources.iter().any(|s| s.same_file(r))) {
        p.steps.push(Step::Delete { path: gone.repo_path(&i.dir), was: ContentHash::of(&gone.bytes) });
    }
}

/// One file the export wants in the setup folder, against what is there now.
fn put(p: &mut Plan, to: PathBuf, bytes: Arc<[u8]>, have: Option<&[u8]>, what: &str) {
    match have {
        Some(old) if old == &*bytes => p.note(NoteKind::Present, format!("{what} is already up to date in the setup folder.")),
        Some(old) => p.steps.push(Step::Write { to, bytes, was: Was::Hash(ContentHash::of(old)) }),
        None => p.steps.push(Step::Write { to, bytes, was: Was::Absent }),
    }
}

/// Rebuilds the setup from a setup folder, local steps first and network steps after, so a failed download
/// cannot strand the links and files. Each step is planned only when something is missing at plan time, so the next
/// plan after a finished restore has no steps. A real folder, a link to somewhere else, a config file that
/// differs or a `.from-backup` file that differs is never replaced. The plan says so and leaves it.
pub(super) fn plan_restore(i: &RestoreInput, p: &mut Plan) {
    for problem in &i.problems {
        p.block(Blocker::Unsupported { why: problem.why.clone(), remedy: problem.remedy.clone() });
    }
    if !p.blockers.is_empty() {
        return;
    }
    let wanted = Wanted::new(&i.manifest, &i.dir, &i.home, &i.repo_config);
    link_own_skills(i, &wanted, p);
    copy_config(i, &wanted, p);
    name_ignored(&i.ignored, p);
    let mut network = plugin_commands(i, p);
    network.extend(skill_commands(i, &wanted, p));
    p.steps.extend(network.into_iter().map(|command| Step::Run { command }));
    block_on_missing_tools(i, p);
    sign_in_notes(p);
}

fn link_own_skills(i: &RestoreInput, wanted: &Wanted, p: &mut Plan) {
    let mut unnamed = BTreeSet::new();
    for name in &i.linked_skills {
        p.note(
            NoteKind::Attention,
            format!("skills/{} is a link, not a folder in the setup folder. It is not linked.", name.as_str()),
        );
    }
    for link in &wanted.own {
        if !i.repo_skills.contains(&link.name) {
            if !i.linked_skills.contains(&link.name) && unnamed.insert(&link.name) {
                p.note(
                    NoteKind::Attention,
                    format!(
                        "Own skill {} is in manifest.json but has no folder under skills/ in the setup folder. It is not linked.",
                        link.name.as_str()
                    ),
                );
            }
            continue;
        }
        let (entry, target) = (link.entry.display(), link.target.display());
        match i.seen.entry(&link.entry) {
            Entry::Absent => p.steps.push(Step::Link { entry: link.entry.clone(), target: link.target.clone(), shelf: None }),
            Entry::LinkTo(to) if norm_key(to) == norm_key(&link.target) => {
                p.note(NoteKind::Present, format!("{entry} already links to {target}."));
            }
            _ => p.note(NoteKind::Attention, format!("{entry} already exists and is not a link to {target}. Left alone.")),
        }
    }
}

fn copy_config(i: &RestoreInput, wanted: &Wanted, p: &mut Plan) {
    for c in &wanted.config {
        let label = c.file.label();
        let bytes = &c.file.bytes;
        if *i.seen.entry(&c.dest) == Entry::Absent {
            p.steps.push(Step::Write { to: c.dest.clone(), bytes: bytes.clone(), was: Was::Absent });
            continue;
        }
        let Some(have) = i.seen.file(&c.dest) else {
            p.note(NoteKind::Attention, format!("{label} exists and is not a plain file. Left alone, and the backup copy is not written."));
            continue;
        };
        if same_text(have, bytes) {
            p.note(NoteKind::Present, format!("{label} already matches the backup."));
            continue;
        }
        let aside = c.aside.display();
        match (i.seen.entry(&c.aside), i.seen.file(&c.aside)) {
            (Entry::Absent, _) => p.steps.push(Step::Write { to: c.aside.clone(), bytes: bytes.clone(), was: Was::Absent }),
            (_, Some(old)) if same_text(old, bytes) => {
                p.note(
                    NoteKind::Attention,
                    format!("{label} differs from the backup. The backup copy is already at {aside}. Compare and merge by hand."),
                );
                continue;
            }
            (_, Some(_)) => {
                let backup = relative_label(i.dir.as_path(), &c.file.repo_path(&i.dir));
                p.note(
                    NoteKind::Attention,
                    format!(
                        "{label} differs from the backup, and {aside} already exists and differs too. It is left alone. The backup copy is {backup} in the setup folder. Compare by hand."
                    ),
                );
                continue;
            }
            (_, None) => {
                p.note(
                    NoteKind::Attention,
                    format!("{label} differs from the backup, and {aside} exists and is not a plain file. Nothing is written."),
                );
                continue;
            }
        }
        p.note(NoteKind::Attention, format!("{label} differs from the backup. The backup copy goes to {aside}. Compare and merge by hand."));
    }
}

/// Equal once Windows line ends are read as Unix ones, so a file git checked out with CRLF is not a difference.
fn same_text(a: &[u8], b: &[u8]) -> bool {
    let lf = |bytes: &[u8]| -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for (at, &byte) in bytes.iter().enumerate() {
            if !(byte == b'\r' && bytes.get(at + 1) == Some(&b'\n')) {
                out.push(byte);
            }
        }
        out
    };
    lf(a) == lf(b)
}

fn name_ignored(ignored: &[String], p: &mut Plan) {
    for name in ignored.iter().take(IGNORED_LISTED) {
        p.note(NoteKind::Attention, format!("{name} is not a file a restore carries. It is ignored and never written."));
    }
    if ignored.len() > IGNORED_LISTED {
        let more = ignored.len() - IGNORED_LISTED;
        p.note(NoteKind::Attention, format!("{more} more files under config/ are ignored and never written."));
    }
}

/// Marketplaces and plugins, each harness's own commands, with what is already there noted.
fn plugin_commands(i: &RestoreInput, p: &mut Plan) -> Vec<ExternalCommand> {
    let none = PluginRecord::default();
    let mut commands = Vec::new();
    for harness in registry() {
        let have = i.plugins.iter().find(|(id, _)| *id == harness.id()).map_or(&none, |(_, record)| record);
        for op in harness.plugin_ops(&i.manifest, have) {
            match op.state {
                PluginState::Present => p.note(NoteKind::Present, format!("{}: {} is already in place.", harness.label(), op.label)),
                PluginState::Missing(command) => commands.push(at_home(command, &i.home)),
            }
        }
    }
    commands
}

/// One skills CLI command per source, for the skills that are not yet in every folder the CLI feeds.
fn skill_commands(i: &RestoreInput, wanted: &Wanted, p: &mut Plan) -> Vec<ExternalCommand> {
    let agents: Vec<&str> = registry().iter().filter_map(|h| h.setup().skills_cli_agent).collect();
    let mut commands = Vec::new();
    for want in &wanted.sources {
        let (have, need): (Vec<_>, Vec<_>) =
            want.skills.iter().partition(|skill| skill.slots.iter().all(|slot| *i.seen.entry(slot) != Entry::Absent));
        if !have.is_empty() {
            let names: Vec<&str> = have.iter().map(|s| s.name.as_str()).collect();
            p.note(NoteKind::Present, format!("Skills from {} are already installed: {}.", want.source.as_str(), names.join(", ")));
        }
        if need.is_empty() {
            continue;
        }
        let mut args: Vec<String> = ["--yes", "skills", "add", want.source.as_str(), "-g", "-a"].map(str::to_owned).into();
        args.extend(agents.iter().map(|a| (*a).to_owned()));
        args.extend(["-y".to_owned(), "--skill".to_owned()]);
        args.extend(need.iter().map(|s| s.name.as_str().to_owned()));
        commands.push(at_home(ExternalCommand { program: SKILLS_CLI.to_owned(), args, cwd: None, env: Vec::new() }, &i.home));
    }
    commands
}

/// A command that reads or writes the user's home runs against the restore's home, so a fixture never reaches the real one.
fn at_home(mut command: ExternalCommand, home: &Path) -> ExternalCommand {
    let home = home.to_string_lossy().into_owned();
    command.env.extend([("HOME".to_owned(), home.clone()), ("USERPROFILE".to_owned(), home)]);
    command
}

/// Only the programs the plan runs are checked. A missing one stops the whole plan before anything is done.
fn block_on_missing_tools(i: &RestoreInput, p: &mut Plan) {
    let programs: BTreeSet<String> = p
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Run { command } => Some(command.program.clone()),
            _ => None,
        })
        .collect();
    for program in programs.iter().filter(|program| !i.seen.has_tool(program)) {
        let hint = registry()
            .iter()
            .map(|h| h.setup())
            .find(|s| s.tools.contains(&program.as_str()))
            .and_then(|s| s.install_hint)
            .unwrap_or(if program == SKILLS_CLI { "Install Node.js from https://nodejs.org. It includes npx." } else { "Install it." });
        p.block(Blocker::Unsupported {
            why: format!("`{program}` is not installed or not on PATH, and this restore has to run it."),
            remedy: format!("{hint} Then plan the restore again."),
        });
    }
}

fn sign_in_notes(p: &mut Plan) {
    for harness in registry() {
        if let Some(how) = harness.setup().sign_in {
            p.note(NoteKind::Manual, format!("Sign in to {}. {how}", harness.label()));
        }
    }
}
