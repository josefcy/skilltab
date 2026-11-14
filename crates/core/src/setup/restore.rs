//! Reads what a restore needs: the setup folder, and this machine at exactly the paths the restore will touch.
//! It writes nothing. `Wanted` is the one list of those paths, so the reader cannot miss one the planner asks about.

use super::{join_rel, own_skills, read_config, repo_name, ConfigFile, Problem, SetupDir};
use crate::fsutil;
use crate::harness::{registry, HarnessSetup, PluginRecord};
use crate::manage::Os;
use crate::model::{norm_key, Arg, FolderName, HarnessId, SetupManifest};
use crate::scan::link_target;
use crate::view::CoreError;
use crate::Env;
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The program that runs the skills CLI. It is not a harness's tool, so no harness lists it.
pub(crate) const SKILLS_CLI: &str = "npx";

const ASIDE_SUFFIX: &str = ".from-backup";

/// Everything the restore planner needs, already read, so planning stays pure.
#[derive(Debug)]
pub(crate) struct RestoreInput {
    pub dir: SetupDir,
    pub home: PathBuf,
    /// Parsed from `<dir>/manifest.json`. Every name in it is already safe to use as an argument or a path segment.
    pub manifest: SetupManifest,
    /// The allowlisted files under `<dir>/config`, never a link and never behind one.
    pub repo_config: Vec<ConfigFile>,
    /// Everything else under `<dir>/config`, as paths relative to the setup folder. Named in the plan, never written.
    pub ignored: Vec<String>,
    /// The folders directly under `<dir>/skills`.
    pub repo_skills: BTreeSet<FolderName>,
    /// What each harness has installed, so `plugin_ops` can tell what is missing.
    pub plugins: Vec<(HarnessId, PluginRecord)>,
    /// An unreadable input. Any one of them blocks the plan, so a half-read machine is never restored over.
    pub problems: Vec<Problem>,
    pub seen: Seen,
}

/// What a path holds, as far as a restore cares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Entry {
    Absent,
    /// A link, and where it points.
    LinkTo(PathBuf),
    /// A folder, a file, or anything the reader could not read. The plan leaves it alone.
    Other,
}

/// What is on this machine at the paths in `Wanted`. A path nobody read is `Other`, so it is never overwritten.
#[derive(Debug, Default)]
pub(crate) struct Seen {
    entries: BTreeMap<PathBuf, Entry>,
    /// Bytes of the plain files among the config destinations and their `.from-backup` siblings.
    files: BTreeMap<PathBuf, Arc<[u8]>>,
    tools: BTreeSet<String>,
}

impl Seen {
    pub(crate) fn entry(&self, path: &Path) -> &Entry {
        static OTHER: Entry = Entry::Other;
        self.entries.get(path).unwrap_or(&OTHER)
    }

    pub(crate) fn file(&self, path: &Path) -> Option<&Arc<[u8]>> {
        self.files.get(path)
    }

    pub(crate) fn has_tool(&self, program: &str) -> bool {
        self.tools.contains(program)
    }

    fn read(wanted: &Wanted, os: &dyn Os) -> Seen {
        let mut seen = Seen::default();
        for path in wanted.paths() {
            let entry = look(&path);
            seen.entries.insert(path, entry);
        }
        for path in wanted.file_paths() {
            if seen.entry(&path) != &Entry::Absent
                && let Some(bytes) = read_plain(&path)
            {
                seen.files.insert(path, bytes);
            }
        }
        seen.tools = candidate_tools().into_iter().filter(|t| os.find_tool(t)).map(str::to_owned).collect();
        seen
    }
}

fn look(path: &Path) -> Entry {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Entry::Absent,
        Ok(meta) if meta.file_type().is_symlink() => link_target(path).map_or(Entry::Other, Entry::LinkTo),
        _ => Entry::Other,
    }
}

fn read_plain(path: &Path) -> Option<Arc<[u8]>> {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).and_then(|_| std::fs::read(path).ok()).map(Into::into)
}

/// The own skill link a restore makes in one harness's skills folder.
#[derive(Debug)]
pub(crate) struct OwnLink {
    pub name: FolderName,
    pub entry: PathBuf,
    /// The folder in the setup repo.
    pub target: PathBuf,
}

/// One skill a source installs, and where each harness that the skills CLI feeds would hold it.
#[derive(Debug)]
pub(crate) struct SkillSlots {
    pub name: FolderName,
    pub slots: Vec<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct SourceWant {
    pub source: Arg,
    pub skills: Vec<SkillSlots>,
}

#[derive(Debug)]
pub(crate) struct ConfigDest {
    pub file: ConfigFile,
    pub dest: PathBuf,
    /// Where the repo copy goes when the destination holds something else.
    pub aside: PathBuf,
}

/// Every place on this machine a restore looks at, worked out from the manifest and the harnesses' static facts.
#[derive(Debug)]
pub(crate) struct Wanted {
    pub own: Vec<OwnLink>,
    pub sources: Vec<SourceWant>,
    pub config: Vec<ConfigDest>,
}

impl Wanted {
    pub(crate) fn new(manifest: &SetupManifest, dir: &SetupDir, home: &Path, repo_config: &[ConfigFile]) -> Wanted {
        let setups: Vec<HarnessSetup> = registry().iter().map(|h| h.setup()).collect();
        let user_dirs = distinct(setups.iter().filter_map(|s| s.skills_dir));
        let cli_dirs = distinct(setups.iter().filter(|s| s.skills_cli_agent.is_some()).filter_map(|s| s.skills_dir));
        let slot = |rel: &str, name: &FolderName| join_rel(home.to_path_buf(), rel).join(name.as_str());

        let mut own = Vec::new();
        for name in &manifest.own_skills {
            for rel in &user_dirs {
                own.push(OwnLink { name: name.clone(), entry: slot(rel, name), target: dir.skills_dir().join(name.as_str()) });
            }
        }
        let sources = manifest
            .skill_sources
            .iter()
            .map(|(source, names)| SourceWant {
                source: source.clone(),
                skills: names
                    .iter()
                    .map(|name| SkillSlots { name: name.clone(), slots: cli_dirs.iter().map(|rel| slot(rel, name)).collect() })
                    .collect(),
            })
            .collect();
        let config = repo_config
            .iter()
            .map(|file| {
                let dest = file.machine_path(home);
                let mut aside = dest.clone().into_os_string();
                aside.push(ASIDE_SUFFIX);
                ConfigDest { file: file.clone(), dest, aside: PathBuf::from(aside) }
            })
            .collect();
        Wanted { own, sources, config }
    }

    /// Every path the plan asks `Seen` about.
    pub(crate) fn paths(&self) -> Vec<PathBuf> {
        let own = self.own.iter().map(|l| l.entry.clone());
        let slots = self.sources.iter().flat_map(|s| &s.skills).flat_map(|k| k.slots.iter().cloned());
        own.chain(slots).chain(self.file_paths()).collect()
    }

    /// The paths whose bytes the plan compares.
    fn file_paths(&self) -> Vec<PathBuf> {
        self.config.iter().flat_map(|c| [c.dest.clone(), c.aside.clone()]).collect()
    }
}

fn distinct<'a>(items: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

/// The programs the plan may run, so `Seen` can say which of them exist. The plan blocks only on the ones it runs.
fn candidate_tools() -> Vec<&'static str> {
    let mut tools = vec![SKILLS_CLI];
    for harness in registry() {
        tools.extend(harness.setup().tools);
    }
    tools
}

/// Err when `<dir>/manifest.json` is absent or not a usable manifest. Both read as "this is not a setup folder".
pub(crate) fn read_restore(env: &Env, os: &dyn Os, dir: SetupDir) -> Result<RestoreInput, CoreError> {
    let manifest = read_manifest(&dir)?;
    let mut scratch = Vec::new();
    let mut plugins = Vec::new();
    for harness in registry() {
        match harness.read_plugins(env) {
            Ok(record) => plugins.push((harness.id(), record)),
            Err(why) => scratch.push(Problem::new(why)),
        }
    }
    let repo_skills = own_skills(&dir, &mut Vec::new());
    let (repo_config, ignored) = read_repo_config(&dir, &mut scratch);
    let wanted = Wanted::new(&manifest, &dir, &env.home, &repo_config);
    let seen = Seen::read(&wanted, os);
    let problems = scratch.into_iter().map(|p| Problem::for_restore(p.why)).collect();
    Ok(RestoreInput { dir, home: env.home.clone(), manifest, repo_config, ignored, repo_skills, plugins, problems, seen })
}

fn read_manifest(dir: &SetupDir) -> Result<SetupManifest, CoreError> {
    let invalid = |message: String| CoreError::Invalid { message };
    let path = dir.manifest_path();
    let text = match fsutil::read_text(&path) {
        Ok(Some(text)) => text,
        Ok(None) => {
            return Err(invalid(format!(
                "This does not look like a setup folder. {} has no manifest.json. Choose the folder you exported a setup to.",
                dir.as_path().display()
            )));
        }
        Err(e) => return Err(invalid(format!("Cannot read {}: {e}.", path.display()))),
    };
    SetupManifest::parse(&text).map_err(|why| {
        invalid(format!("This does not look like a setup folder. {} is not a usable manifest. {why}", path.display()))
    })
}

/// The allowlisted files under `<dir>/config`, and the names of every other file there. A file behind a link is
/// ignored too, so a cloned repo cannot point the allowlist at a file outside itself.
fn read_repo_config(dir: &SetupDir, problems: &mut Vec<Problem>) -> (Vec<ConfigFile>, Vec<String>) {
    let mut carried = Vec::new();
    for setup in registry().iter().map(|h| h.setup()).filter(|s| !s.config_home.is_empty()) {
        carried.extend(read_config(&dir.config_dir().join(repo_name(setup.config_home)), &setup, problems));
    }
    let (plain, linked): (Vec<ConfigFile>, Vec<ConfigFile>) =
        carried.into_iter().partition(|file| is_plain(dir.as_path(), &file.repo_path(dir)));
    let plain_keys: BTreeSet<String> = plain.iter().map(|f| norm_key(&f.repo_path(dir))).collect();

    let mut every = Vec::new();
    walk_files(&dir.config_dir(), &mut every, problems);
    every.extend(linked.iter().map(|f| f.repo_path(dir)));
    let mut ignored: Vec<String> =
        every.iter().filter(|p| !plain_keys.contains(&norm_key(p))).map(|p| relative_label(dir.as_path(), p)).collect();
    ignored.sort();
    ignored.dedup();
    (plain, ignored)
}

/// True when no part of `path` below `root` is a link.
fn is_plain(root: &Path, path: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(root) else { return false };
    let mut walked = root.to_path_buf();
    rel.components().all(|part| {
        walked.push(part);
        std::fs::symlink_metadata(&walked).is_ok_and(|m| !m.file_type().is_symlink())
    })
}

/// Every entry below `folder` that is not a folder, links included, and none of the folders a link leads to.
fn walk_files(folder: &Path, out: &mut Vec<PathBuf>, problems: &mut Vec<Problem>) {
    let read = match std::fs::read_dir(folder) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => return problems.push(Problem::new(format!("Could not read {}: {e}.", folder.display()))),
    };
    let mut items: Vec<_> = read.flatten().collect();
    items.sort_by_key(|i| i.file_name());
    for item in items {
        match item.file_type() {
            Ok(kind) if kind.is_dir() => walk_files(&item.path(), out, problems),
            _ => out.push(item.path()),
        }
    }
}

fn relative_label(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manage::tests::FakeOs;
    use crate::testutil::Home;
    use std::fs;

    const MANIFEST: &str = r#"{
      "marketplaces": [{ "name": "alpha", "source": { "source": "github", "repo": "acme/alpha" } }],
      "claudePlugins": ["plug@alpha"],
      "skillSources": [{ "source": "acme/skills", "skills": ["s1", "s2"] }],
      "ownSkills": ["own1", "gone"]
    }"#;

    fn setup_folder(h: &Home) {
        fs::create_dir_all(h.p("repo/skills/own1")).unwrap();
        fs::write(h.p("repo/manifest.json"), MANIFEST).unwrap();
        fs::create_dir_all(h.p("repo/config/claude/rules")).unwrap();
        fs::write(h.p("repo/config/claude/CLAUDE.md"), "backup").unwrap();
        fs::write(h.p("repo/config/claude/rules/r.md"), "rule").unwrap();
        fs::write(h.p("repo/config/claude/settings.json"), "{}").unwrap();
        fs::create_dir_all(h.p("repo/config/codex")).unwrap();
        fs::write(h.p("repo/config/codex/AGENTS.md"), "agents").unwrap();
    }

    fn read(h: &Home, os: &FakeOs) -> Result<RestoreInput, CoreError> {
        let env = h.env();
        read_restore(&env, os, SetupDir::parse(&env, "~/repo").unwrap())
    }

    fn message(e: CoreError) -> String {
        match e {
            CoreError::Invalid { message } => message,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn it_reads_the_manifest_the_allowlisted_files_and_the_own_skill_folders() {
        let h = Home::new();
        setup_folder(&h);
        let input = read(&h, &FakeOs::default()).unwrap();

        assert!(input.problems.is_empty(), "{:?}", input.problems);
        assert_eq!(input.manifest.own_skills.len(), 2);
        let labels: Vec<String> = input.repo_config.iter().map(ConfigFile::label).collect();
        assert_eq!(labels, ["~/.claude/CLAUDE.md", "~/.claude/rules/r.md", "~/.codex/AGENTS.md"]);
        let own: Vec<&str> = input.repo_skills.iter().map(FolderName::as_str).collect();
        assert_eq!(own, ["own1"], "a manifest name with no folder is for the planner to name");
        assert_eq!(input.plugins.len(), 2, "one record per harness");
    }

    #[test]
    fn a_file_outside_the_allowlist_is_named_and_never_carried() {
        let h = Home::new();
        setup_folder(&h);
        fs::create_dir_all(h.p("repo/config/ssh")).unwrap();
        fs::write(h.p("repo/config/ssh/id_rsa"), "key").unwrap();
        fs::write(h.p("repo/config/claude/rules/notes.txt"), "x").unwrap();
        let input = read(&h, &FakeOs::default()).unwrap();

        assert_eq!(input.ignored, ["config/claude/rules/notes.txt", "config/claude/settings.json", "config/ssh/id_rsa"]);
        assert!(input.repo_config.iter().all(|f| !f.rel.contains("id_rsa") && !f.rel.contains("settings")));
        let wanted = Wanted::new(&input.manifest, &input.dir, &input.home, &input.repo_config);
        assert!(wanted.paths().iter().all(|p| !p.to_string_lossy().contains("ssh")));
    }

    #[test]
    fn a_folder_that_is_a_link_inside_config_is_ignored_with_everything_behind_it() {
        let h = Home::new();
        setup_folder(&h);
        fs::create_dir_all(h.p("elsewhere")).unwrap();
        fs::write(h.p("elsewhere/outside.md"), "outside").unwrap();
        fs::remove_dir_all(h.p("repo/config/claude/rules")).unwrap();
        h.link("repo/config/claude/rules", "elsewhere");
        let input = read(&h, &FakeOs::default()).unwrap();

        assert!(input.repo_config.iter().all(|f| !f.rel.starts_with("rules")), "{:?}", input.repo_config);
        assert!(input.ignored.contains(&"config/claude/rules/outside.md".to_owned()), "{:?}", input.ignored);
    }

    #[test]
    fn a_folder_without_a_usable_manifest_is_not_a_setup_folder() {
        let h = Home::new();
        fs::create_dir_all(h.p("repo")).unwrap();
        assert!(message(read(&h, &FakeOs::default()).unwrap_err()).contains("does not look like a setup folder"));

        let hostile = [
            "not json",
            r#"{"claudePlugins":["--scope@x"]}"#,
            r#"{"ownSkills":["../up"]}"#,
            r#"{"ownSkills":["a/b"]}"#,
            r#"{"skillSources":[{"source":"acme/skills","skills":["--yes"]}]}"#,
        ];
        for text in hostile {
            fs::write(h.p("repo/manifest.json"), text).unwrap();
            let why = message(read(&h, &FakeOs::default()).unwrap_err());
            assert!(why.contains("does not look like a setup folder") && why.contains("manifest.json"), "{text}: {why}");
        }
    }

    #[test]
    fn seen_holds_exactly_the_wanted_paths_and_a_path_nobody_read_is_other() {
        let h = Home::new();
        setup_folder(&h);
        fs::create_dir_all(h.p(".agents/skills/s1")).unwrap();
        h.link(".claude/skills/own1", "repo/skills/own1");
        fs::create_dir_all(h.p(".claude")).unwrap();
        fs::write(h.p(".claude/CLAUDE.md"), "mine").unwrap();
        fs::write(h.p(".claude/CLAUDE.md.from-backup"), "old").unwrap();
        fs::write(h.p("unrelated"), "x").unwrap();
        let input = read(&h, &FakeOs::default()).unwrap();
        let seen = &input.seen;

        assert_eq!(seen.entry(&h.p(".claude/skills/own1")), &Entry::LinkTo(h.p("repo/skills/own1")));
        assert_eq!(seen.entry(&h.p(".agents/skills/own1")), &Entry::Absent);
        assert_eq!(seen.entry(&h.p(".agents/skills/s1")), &Entry::Other);
        assert_eq!(seen.entry(&h.p(".claude/skills/s1")), &Entry::Absent);
        assert_eq!(&**seen.file(&h.p(".claude/CLAUDE.md")).unwrap(), b"mine");
        assert_eq!(&**seen.file(&h.p(".claude/CLAUDE.md.from-backup")).unwrap(), b"old");
        assert!(seen.file(&h.p(".claude/rules/r.md")).is_none());
        assert_eq!(seen.entry(&h.p(".claude/rules/r.md")), &Entry::Absent);

        assert_eq!(seen.entry(&h.p("unrelated")), &Entry::Other, "a path the reader never looked at is never treated as free");
        assert!(seen.file(&h.p("unrelated")).is_none());
        let wanted = Wanted::new(&input.manifest, &input.dir, &input.home, &input.repo_config);
        let asked: BTreeSet<PathBuf> = wanted.paths().into_iter().collect();
        assert_eq!(seen.entries.keys().cloned().collect::<BTreeSet<_>>(), asked);
    }

    #[test]
    fn every_harness_skills_folder_gets_one_slot_per_own_skill_and_per_installed_skill() {
        let h = Home::new();
        setup_folder(&h);
        let input = read(&h, &FakeOs::default()).unwrap();
        let wanted = Wanted::new(&input.manifest, &input.dir, &input.home, &input.repo_config);

        let own: Vec<PathBuf> = wanted.own.iter().map(|l| l.entry.clone()).collect();
        assert_eq!(own, [".claude/skills/gone", ".agents/skills/gone", ".claude/skills/own1", ".agents/skills/own1"].map(|p| h.p(p)));
        assert!(wanted.own.iter().all(|l| l.target == h.p(&format!("repo/skills/{}", l.name.as_str()))));
        let slots: Vec<PathBuf> = wanted.sources[0].skills[0].slots.clone();
        assert_eq!(slots, [".claude/skills/s1", ".agents/skills/s1"].map(|p| h.p(p)));
    }

    #[test]
    fn it_asks_the_os_only_about_programs_a_restore_can_run() {
        let h = Home::new();
        setup_folder(&h);
        let input = read(&h, &FakeOs::with_tools(&["claude", "npx", "git", "unrelated"])).unwrap();
        assert!(input.seen.has_tool("claude") && input.seen.has_tool("npx"));
        assert!(!input.seen.has_tool("git") && !input.seen.has_tool("unrelated"));
        assert!(!read(&h, &FakeOs::default()).unwrap().seen.has_tool("claude"));
    }
}
