//! Reads what a setup backup is made of and defines its format. It writes nothing: `manage` does.
//!
//! The setup folder is the one path the UI sends. `SetupDir::parse` is the only place that string becomes a
//! path. Everything else here reads files the harnesses and the skill lock name, never a path from the UI.

mod manifest;
mod restore;
mod secret;

use crate::fsutil;
use crate::harness::{registry, Carry, HarnessSetup};
use crate::model::{norm_key, FolderName, SetupManifest};
use crate::provenance::Provenance;
use crate::view::CoreError;
use crate::Env;
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) use restore::{read_restore, Entry, RestoreInput, Wanted, SKILLS_CLI};
pub(crate) use secret::find_secret;

/// An existing, absolute, canonical folder. Only `parse` makes one.
#[derive(Clone, Debug)]
pub(crate) struct SetupDir(PathBuf);

impl SetupDir {
    /// A leading `~` is the home folder. Anything else must already be an absolute path to a folder that exists.
    /// The error is a sentence for the user.
    pub(crate) fn parse(env: &Env, raw: &str) -> Result<SetupDir, CoreError> {
        let invalid = |message: String| CoreError::Invalid { message };
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(invalid("Type the folder of your setup repo.".to_owned()));
        }
        let path = match raw.strip_prefix('~') {
            Some("") => env.home.clone(),
            Some(rest) if rest.starts_with(std::path::is_separator) => {
                rest.split(std::path::is_separator).filter(|c| !c.is_empty()).fold(env.home.clone(), |p, c| p.join(c))
            }
            _ => PathBuf::from(raw),
        };
        if !path.is_absolute() {
            return Err(invalid(format!("{raw} is not a full path. Type the whole path, or start it with ~ for your home folder.")));
        }
        let canonical = std::fs::canonicalize(&path).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => invalid(format!("The folder {} does not exist.", path.display())),
            _ => invalid(format!("Cannot open {}: {e}.", path.display())),
        })?;
        if !canonical.is_dir() {
            return Err(invalid(format!("{} is a file, not a folder.", path.display())));
        }
        Ok(SetupDir(fsutil::strip_verbatim(canonical)))
    }

    pub(crate) fn as_path(&self) -> &Path {
        &self.0
    }
    pub(crate) fn manifest_path(&self) -> PathBuf {
        self.0.join("manifest.json")
    }
    pub(crate) fn config_dir(&self) -> PathBuf {
        self.0.join("config")
    }
    pub(crate) fn skills_dir(&self) -> PathBuf {
        self.0.join("skills")
    }
}

/// One allowlisted file. The same file has a place on the machine and a place in the setup folder, and both
/// come from `home_dir` and `rel`.
#[derive(Clone, Debug)]
pub(crate) struct ConfigFile {
    /// The harness's folder under the home folder, such as ".claude".
    pub home_dir: &'static str,
    /// Relative to `home_dir`, with `/` separators.
    pub rel: String,
    pub bytes: Arc<[u8]>,
}

impl ConfigFile {
    /// The file as the user knows it, such as `~/.claude/rules/a.md`.
    pub(crate) fn label(&self) -> String {
        format!("~/{}/{}", self.home_dir, self.rel)
    }

    /// `<setup folder>/config/<harness folder without the dot>/<rel>`.
    pub(crate) fn repo_path(&self, dir: &SetupDir) -> PathBuf {
        join_rel(dir.config_dir().join(repo_name(self.home_dir)), &self.rel)
    }

    /// Where the file lives on this machine, such as `<home>/.claude/rules/a.md`.
    pub(crate) fn machine_path(&self, home: &Path) -> PathBuf {
        join_rel(home.join(self.home_dir), &self.rel)
    }

    pub(crate) fn same_file(&self, other: &ConfigFile) -> bool {
        self.home_dir == other.home_dir && self.rel == other.rel
    }
}

fn repo_name(home_dir: &str) -> &str {
    home_dir.trim_start_matches('.')
}

fn join_rel(base: PathBuf, rel: &str) -> PathBuf {
    rel.split('/').fold(base, |p, c| p.join(c))
}

/// Something that stops an export, with what to do about it.
#[derive(Clone, Debug)]
pub(crate) struct Problem {
    pub why: String,
    pub remedy: String,
}

impl Problem {
    fn new(why: String) -> Problem {
        Problem { why, remedy: "Fix that, then plan the export again.".to_owned() }
    }

    fn for_restore(why: String) -> Problem {
        Problem { why, remedy: "Fix that, then plan the restore again.".to_owned() }
    }
}

/// Everything the export planner needs, already read, so planning stays pure.
#[derive(Debug)]
pub(crate) struct ExportInput {
    pub dir: SetupDir,
    /// What this machine says, plus the own skills found under `<dir>/skills`.
    pub manifest: SetupManifest,
    /// Known marketplaces the format cannot rebuild, as (name, source type). Export leaves them out and says so.
    pub unrebuildable: Vec<(String, String)>,
    /// An unreadable input, or a destination Export refuses. Any one of them blocks the plan.
    pub problems: Vec<Problem>,
    pub on_disk_manifest: Option<Arc<[u8]>>,
    /// The allowlisted files on this machine.
    pub sources: Vec<ConfigFile>,
    /// The allowlist shaped files already under `<dir>/config`.
    pub in_repo: Vec<ConfigFile>,
}

pub(crate) fn read_export(env: &Env, dir: SetupDir) -> ExportInput {
    let mut problems = Vec::new();
    let mut manifest = SetupManifest::default();
    let mut unrebuildable = Vec::new();

    for harness in registry() {
        match harness.read_plugins(env) {
            Ok(record) => {
                manifest.marketplaces.extend(record.marketplaces);
                manifest.plugins.extend(record.enabled);
                unrebuildable.extend(record.uncarried);
            }
            Err(why) => problems.push(Problem::new(why)),
        }
    }
    match Provenance::load(env).skill_sources() {
        Ok(sources) => manifest.skill_sources = sources,
        Err(why) => problems.push(Problem::new(why)),
    }
    manifest.own_skills = own_skills(&dir, &mut problems);
    let on_disk_manifest = read_file(&dir.manifest_path(), &mut problems);

    let setups: Vec<HarnessSetup> = registry().iter().map(|h| h.setup()).filter(|s| !s.config_home.is_empty()).collect();
    let (mut sources, mut in_repo) = (Vec::new(), Vec::new());
    let links = link_problems(&dir);
    let linked = !links.is_empty();
    problems.extend(links);
    for setup in &setups {
        sources.extend(read_config(&env.home.join(setup.config_home), setup, &mut problems));
        if !linked {
            in_repo.extend(read_config(&dir.config_dir().join(repo_name(setup.config_home)), setup, &mut problems));
        }
    }
    if let Some(folder) = guarded_folder(env, &dir, &setups) {
        problems.push(Problem {
            why: format!("The setup folder is inside ~/{folder}, which a harness owns. A backup there would sit inside the thing it backs up."),
            remedy: "Choose a folder outside ~/.claude, ~/.codex and ~/.agents.".to_owned(),
        });
    }
    ExportInput { dir, manifest, unrebuildable, problems, on_disk_manifest, sources, in_repo }
}

/// How many links an export names one by one before it sums up the rest.
const LINKS_NAMED: usize = 10;

/// A problem for each link at or below `<dir>/manifest.json` and `<dir>/config`, none of them followed. Export
/// writes and deletes there, so a link in a cloned folder would carry it outside the setup folder.
fn link_problems(dir: &SetupDir) -> Vec<Problem> {
    fn walk(path: &Path, out: &mut Vec<PathBuf>, problems: &mut Vec<Problem>) {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => out.push(path.to_path_buf()),
            Ok(meta) if meta.is_dir() => match std::fs::read_dir(path) {
                Ok(read) => {
                    let mut items: Vec<PathBuf> = read.flatten().map(|i| i.path()).collect();
                    items.sort();
                    items.iter().for_each(|item| walk(item, out, problems));
                }
                Err(e) => problems.push(Problem::new(format!("Could not read {}: {e}.", path.display()))),
            },
            _ => {}
        }
    }
    let (mut links, mut problems) = (Vec::new(), Vec::new());
    walk(&dir.manifest_path(), &mut links, &mut problems);
    walk(&dir.config_dir(), &mut links, &mut problems);
    let remedy = "Remove the link or pick another folder.".to_owned();
    let named = |path: &Path| format!("{} is a link. skilltab will not write through a link.", relative_label(dir.as_path(), path));
    problems.extend(links.iter().take(LINKS_NAMED).map(|path| Problem { why: named(path), remedy: remedy.clone() }));
    if links.len() > LINKS_NAMED {
        let more = links.len() - LINKS_NAMED;
        problems.push(Problem { why: format!("{more} more links are in the setup folder. skilltab will not write through a link."), remedy });
    }
    problems
}

fn relative_label(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// The folder under the home folder that holds `dir`, if it is one a harness or the skills CLI owns.
fn guarded_folder(env: &Env, dir: &SetupDir, setups: &[HarnessSetup]) -> Option<String> {
    let key = norm_key(dir.as_path());
    let top = |rel: &str| rel.split('/').next().unwrap_or_default().to_owned();
    let names = setups.iter().flat_map(|s| std::iter::once(top(s.config_home)).chain(s.skills_dir.map(top)));
    for name in names {
        let root = env.home.join(&name);
        let root = fsutil::strip_verbatim(std::fs::canonicalize(&root).unwrap_or(root));
        let root = norm_key(&root);
        if key == root || key.starts_with(&format!("{root}{}", std::path::MAIN_SEPARATOR)) {
            return Some(name);
        }
    }
    None
}

/// Folders directly under `<dir>/skills`, as `backup.ps1` lists them. A dot folder is hidden and skipped.
fn own_skills(dir: &SetupDir, problems: &mut Vec<Problem>) -> BTreeSet<FolderName> {
    let root = dir.skills_dir();
    let mut out = BTreeSet::new();
    let read = match std::fs::read_dir(&root) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return out,
        Err(e) => {
            problems.push(Problem::new(format!("Could not read {}: {e}.", root.display())));
            return out;
        }
    };
    for item in read.flatten().filter(|i| i.path().is_dir()) {
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            problems.push(Problem::new(format!("A folder in {} has a name that is not valid text.", root.display())));
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        match FolderName::parse(&name) {
            Ok(n) => {
                out.insert(n);
            }
            Err(e) => problems.push(Problem::new(format!("An own skill folder under {} is not usable: {e}.", root.display()))),
        }
    }
    out
}

/// The bytes of a file. A path that is absent or is not a file reads as None.
fn read_file(path: &Path, problems: &mut Vec<Problem>) -> Option<Arc<[u8]>> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => match std::fs::read(path) {
            Ok(bytes) => Some(bytes.into()),
            Err(e) => {
                problems.push(Problem::new(format!("Could not read {}: {e}.", path.display())));
                None
            }
        },
        Ok(_) => None,
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            problems.push(Problem::new(format!("Could not read {}: {e}.", path.display())));
            None
        }
    }
}

/// The allowlisted files under `root`, which is the harness folder on the machine or its copy in the setup folder.
fn read_config(root: &Path, setup: &HarnessSetup, problems: &mut Vec<Problem>) -> Vec<ConfigFile> {
    let mut out = Vec::new();
    let mut push = |rel: String, bytes: Arc<[u8]>| out.push(ConfigFile { home_dir: setup.config_home, rel, bytes });
    for carry in setup.carry {
        match carry {
            Carry::File(name) => {
                if let Some(bytes) = read_file(&join_rel(root.to_path_buf(), name), problems) {
                    push((*name).to_owned(), bytes);
                }
            }
            Carry::Dir { dir, ext } => {
                let folder = join_rel(root.to_path_buf(), dir);
                for name in names_with_extension(&folder, ext, problems) {
                    if let Some(bytes) = read_file(&folder.join(&name), problems) {
                        push(format!("{dir}/{name}"), bytes);
                    }
                }
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// Sorted names of the files directly in `folder` that end in `.ext`, in any case. Dot files are hidden and skipped.
fn names_with_extension(folder: &Path, ext: &str, problems: &mut Vec<Problem>) -> Vec<String> {
    let read = match std::fs::read_dir(folder) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            problems.push(Problem::new(format!("Could not read {}: {e}.", folder.display())));
            return Vec::new();
        }
    };
    let mut names = Vec::new();
    for item in read.flatten() {
        let path = item.path();
        if !path.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) || !path.is_file() {
            continue;
        }
        match item.file_name().to_str() {
            Some(name) if !name.starts_with('.') => names.push(name.to_owned()),
            Some(_) => {}
            None => problems.push(Problem::new(format!("A file in {} has a name that is not valid text.", folder.display()))),
        }
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{copy_tree, Home};
    use std::fs;

    fn fixture(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/setup").join(rel)
    }

    fn invalid(e: CoreError) -> String {
        match e {
            CoreError::Invalid { message } => message,
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn a_leading_tilde_is_the_home_folder() {
        let h = Home::new();
        fs::create_dir_all(h.p("backup/sub")).unwrap();
        let env = h.env();
        let dir = |raw: &str| SetupDir::parse(&env, raw).unwrap().as_path().to_path_buf();

        assert_eq!(dir("~"), h.root);
        assert_eq!(dir("~/backup"), h.p("backup"));
        assert_eq!(dir("  ~/backup/sub/  "), h.p("backup/sub"));
        assert_eq!(dir("~/backup/../backup"), h.p("backup"), "the folder is canonical");
        #[cfg(windows)]
        assert_eq!(dir(r"~\backup"), h.p("backup"));
    }

    #[test]
    fn a_folder_that_is_not_a_full_path_to_an_existing_folder_is_refused_with_a_sentence() {
        let h = Home::new();
        fs::write(h.p("file.txt"), "x").unwrap();
        let env = h.env();
        let refuse = |raw: &str| invalid(SetupDir::parse(&env, raw).unwrap_err());

        assert!(refuse("").contains("Type the folder"));
        assert!(refuse("   ").contains("Type the folder"));
        assert!(refuse("backup").contains("not a full path"));
        assert!(refuse("./backup").contains("not a full path"));
        assert!(refuse("~backup").contains("not a full path"), "only a leading ~ and a separator is home");
        assert!(refuse(&h.p("missing").to_string_lossy()).contains("does not exist"));
        assert!(refuse("~/missing").contains("does not exist"));
        assert!(refuse(&h.p("file.txt").to_string_lossy()).contains("is a file"));
    }

    fn seeded_export() -> (Home, ExportInput) {
        let h = Home::new();
        copy_tree(&fixture("home"), &h.root);
        copy_tree(&fixture("expected/skills"), &h.p("repo/skills"));
        fs::write(h.p(".claude/hooks.json"), "{}").unwrap();
        fs::write(h.p(".codex/auth.json"), "{}").unwrap();
        let env = h.env();
        let input = read_export(&env, SetupDir::parse(&env, "~/repo").unwrap());
        (h, input)
    }

    #[test]
    fn export_reads_the_allowlisted_files_and_nothing_else() {
        let (_h, input) = seeded_export();
        assert!(input.problems.is_empty(), "{:?}", input.problems);
        let labels: Vec<String> = input.sources.iter().map(ConfigFile::label).collect();
        assert_eq!(labels, ["~/.claude/CLAUDE.md", "~/.claude/rules/a-rule.md", "~/.claude/rules/b-rule.md", "~/.codex/AGENTS.md"]);
        assert!(input.in_repo.is_empty() && input.on_disk_manifest.is_none());
    }

    #[test]
    fn export_reads_the_manifest_the_machine_describes() {
        let (_h, input) = seeded_export();
        let expected = SetupManifest::parse(&fs::read_to_string(fixture("expected/manifest.json")).unwrap()).unwrap();
        assert_eq!(input.manifest, expected);
        assert_eq!(input.unrebuildable, [("local-market".to_owned(), "directory".to_owned())]);
    }

    #[test]
    fn export_refuses_a_destination_inside_a_harness_folder() {
        for inside in [".claude", ".claude/backup", ".codex/x", ".agents"] {
            let h = Home::new();
            fs::create_dir_all(h.p(inside)).unwrap();
            let env = h.env();
            let input = read_export(&env, SetupDir::parse(&env, &format!("~/{inside}")).unwrap());
            assert!(input.problems.iter().any(|p| p.why.contains("inside ~/.")), "{inside}: {:?}", input.problems);
        }
        let h = Home::new();
        fs::create_dir_all(h.p(".claude-backup")).unwrap();
        let env = h.env();
        let input = read_export(&env, SetupDir::parse(&env, "~/.claude-backup").unwrap());
        assert!(input.problems.is_empty(), "a sibling that only shares a prefix is fine: {:?}", input.problems);
    }

    #[test]
    fn export_names_an_own_skill_folder_it_cannot_put_in_a_manifest() {
        let h = Home::new();
        fs::create_dir_all(h.p("repo/skills/ok")).unwrap();
        fs::create_dir_all(h.p("repo/skills/.hidden")).unwrap();
        fs::create_dir_all(h.p("repo/skills/-flag")).unwrap();
        fs::write(h.p("repo/skills/file.txt"), "x").unwrap();
        let env = h.env();
        let input = read_export(&env, SetupDir::parse(&env, "~/repo").unwrap());
        let own: Vec<&str> = input.manifest.own_skills.iter().map(FolderName::as_str).collect();
        assert_eq!(own, ["ok"]);
        assert_eq!(input.problems.len(), 1);
        assert!(input.problems[0].why.contains("-flag"), "{:?}", input.problems);
    }
}
