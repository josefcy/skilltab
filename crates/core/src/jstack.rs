//! The link to the user's jstack repo. skilltab reads the clone and offers jstack's own scripts.
//! It does not re-implement jstack's linking policy (which harness gets which link, exclude.txt,
//! aliases.txt), never writes ~/.jstack-links.txt, never edits the repo, and never pushes.
//!
//! Repo layout relied on: skills/<name>/SKILL.md (own skills), skill-sources.txt
//! ("<source> <skill> <skill> ..."), exclude.txt, sync-skills.ps1.
//!
//! `RepoState::load` reads the clone once per refresh, and nothing here touches the network
//! except `fetch`, which only runs when the user asks.

mod git;

use crate::fsutil;
use crate::model::*;
use crate::scan::content_hash_of;
use std::fmt;
use std::path::{Path, PathBuf};

/// Everything skilltab knows about the clone. Relations and drift are pure functions of this and a `Snapshot`.
#[derive(Debug, Clone)]
pub struct RepoState {
    pub root: PathBuf,
    pub remote: Option<String>,
    pub head: String,
    /// None on a detached HEAD.
    pub branch: Option<String>,
    /// None when the branch has no upstream, or when its remote-tracking ref is gone.
    pub upstream: Option<Upstream>,
    /// Repo-relative paths under `skills/` that differ from HEAD, untracked files included.
    pub dirty: Vec<PathBuf>,
    /// When `git fetch` or `git pull` last wrote FETCH_HEAD. The ahead/behind counts are as old as this.
    pub fetched_at: Option<Timestamp>,
    /// Own skills as they are in the working tree, which is what sync-skills.ps1 links.
    pub skills: Vec<RepoSkill>,
    pub declared: Vec<DeclaredSource>,
    /// exclude.txt: names that sync-skills.ps1 never mirrors between harnesses.
    pub excluded: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    pub name: String,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Debug, Clone)]
pub struct RepoSkill {
    /// The folder name under `skills/`. This is also what `Origin::JstackRepo` carries.
    pub name: String,
    pub dir: PathBuf,
    pub content_hash: ContentHash,
}

/// One line of skill-sources.txt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredSource {
    pub source: String,
    pub skills: Vec<String>,
}

#[derive(Debug)]
pub enum JstackError {
    NotFound(PathBuf),
    NotAGitRepo(PathBuf),
    Git { command: String, message: String },
    Io { path: PathBuf, error: String },
}

impl fmt::Display for JstackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JstackError::NotFound(p) => write!(f, "{} does not exist", p.display()),
            JstackError::NotAGitRepo(p) => write!(f, "{} is not the root of a git repository", p.display()),
            JstackError::Git { command, message } => write!(f, "`{command}` failed: {message}"),
            JstackError::Io { path, error } => write!(f, "{}: {error}", path.display()),
        }
    }
}

impl std::error::Error for JstackError {}

impl RepoState {
    /// Read-only. Four short `git` calls plus the repo's own files. Never fetches.
    pub fn load(root: &Path) -> Result<RepoState, JstackError> {
        if !root.is_dir() {
            return Err(JstackError::NotFound(root.to_path_buf()));
        }
        let prefix = git::run(root, &["rev-parse", "--show-prefix"])?;
        if !prefix.trim().is_empty() {
            return Err(JstackError::NotAGitRepo(root.to_path_buf()));
        }
        let status = git::parse_status(&git::run(
            root,
            &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all", "--", "skills"],
        )?);
        let head = status.head.ok_or_else(|| JstackError::Git {
            command: "git status".into(),
            message: "the repository has no commits".into(),
        })?;
        let remote = git::run_optional(root, &["config", "--get", "remote.origin.url"])?;
        let fetch_head = git::run(root, &["rev-parse", "--git-path", "FETCH_HEAD"])?;

        Ok(RepoState {
            root: root.to_path_buf(),
            remote,
            head,
            branch: status.branch,
            upstream: status.upstream.map(|(name, ahead, behind)| Upstream { name, ahead, behind }),
            dirty: status.dirty.into_iter().map(PathBuf::from).collect(),
            fetched_at: modified_at(&root.join(fetch_head.trim())),
            skills: read_skills(root)?,
            declared: parse_sources(&read_list(&root.join("skill-sources.txt"))?),
            excluded: read_list(&root.join("exclude.txt"))?,
        })
    }
}

/// `git fetch`. The one call that touches the network, so it is its own function and runs only on
/// an explicit request. It never changes the working tree. Reload with `RepoState::load` afterwards.
pub fn fetch(root: &Path) -> Result<(), JstackError> {
    git::run(root, &["fetch", "--quiet"]).map(drop)
}

fn modified_at(path: &Path) -> Option<Timestamp> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok().map(Timestamp::from)
}

fn read_skills(root: &Path) -> Result<Vec<RepoSkill>, JstackError> {
    let dir = root.join("skills");
    let read = match std::fs::read_dir(&dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(JstackError::Io { path: dir, error: e.to_string() }),
    };
    let mut skills: Vec<RepoSkill> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("SKILL.md").is_file())
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?.to_owned();
            Some(RepoSkill { name, content_hash: content_hash_of(&p), dir: p })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// A line-per-entry file with `#` comments. A missing file is an empty list.
fn read_list(path: &Path) -> Result<Vec<String>, JstackError> {
    let text = fsutil::read_text(path)
        .map_err(|e| JstackError::Io { path: path.to_path_buf(), error: e.to_string() })?
        .unwrap_or_default();
    Ok(text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).map(str::to_owned).collect())
}

fn parse_sources(lines: &[String]) -> Vec<DeclaredSource> {
    lines
        .iter()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            let source = parts.next()?.to_owned();
            Some(DeclaredSource { source, skills: parts.map(str::to_owned).collect() })
        })
        .collect()
}

// ---------------------------------------------------------------- relations and drift

/// How one installed body relates to the repo. Derived on every call, never stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JstackRelation {
    /// The body is the repo folder itself, reached through a junction. It cannot differ from the
    /// working tree, so drift is about the working tree against HEAD and HEAD against upstream.
    Owned { skill: String, drift: Drift },
    /// A separate folder with the same name as a repo skill. Drift is a content-hash comparison.
    CopyOf { skill: String, drift: Drift },
    /// Listed in skill-sources.txt and installed by the `skills` CLI.
    Declared { source: String, skill: String },
}

/// Only one drift is reported, the most urgent: edits first because they are the user's unsaved
/// work, then a diverged branch because `git pull --ff-only` cannot fix it, then plain behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    InSync,
    /// Owned: files under the skill's folder differ from HEAD.
    UncommittedEdits { files: Vec<PathBuf> },
    /// Owned: upstream has commits that HEAD lacks. Counted across the whole repo, as of `fetched_at`.
    Behind { commits: u32 },
    /// Owned: local commits and upstream commits both exist, so a fast-forward pull fails.
    Diverged { ahead: u32, behind: u32 },
    /// CopyOf: the folder's bytes differ from the repo folder's.
    ContentDiffers { installed: ContentHash, repo: ContentHash },
}

/// A repo skill that a present harness does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotInstalled {
    pub harness: HarnessId,
    pub skill: String,
    pub why: Absence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Absence {
    /// Nothing in the harness answers to the name. Running sync-skills.ps1 links it.
    Absent,
    /// Another body already uses the name there. sync-skills.ps1 keeps that one and skips the repo's.
    NameTaken { by: SkillId },
}

/// A skill that skill-sources.txt lists and no installed `skills` CLI body provides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredMissing {
    pub source: String,
    pub skill: String,
}

#[derive(Debug, Clone, Default)]
pub struct JstackReport {
    /// Bodies that relate to the repo, ordered by skill name.
    pub relations: Vec<(SkillId, JstackRelation)>,
    /// Ordered by harness, then skill. Excluded names (exclude.txt) are never listed.
    pub not_installed: Vec<NotInstalled>,
    pub declared_missing: Vec<DeclaredMissing>,
}

/// Pure. `Owned` wins over `CopyOf`, which wins over `Declared`.
pub fn relate(body: &SkillBody, repo: &RepoState) -> Option<JstackRelation> {
    if let Origin::JstackRepo { skill } = &body.origin {
        return Some(JstackRelation::Owned { skill: skill.clone(), drift: owned_drift(repo, skill) });
    }
    // A plugin, claude.ai sync or harness built-in that merely shares a name is a clash, not a copy
    // the user can bring back in line with the repo. Report it through `NotInstalled`'s `NameTaken`.
    let foreign = matches!(body.origin, Origin::Plugin { .. } | Origin::ClaudeAiSync { .. } | Origin::HarnessBuiltin { .. });
    if let Some(own) = repo.skills.iter().find(|s| !foreign && answers_to(body, &s.name)) {
        let drift = if body.content_hash == own.content_hash {
            Drift::InSync
        } else {
            Drift::ContentDiffers { installed: body.content_hash.clone(), repo: own.content_hash.clone() }
        };
        return Some(JstackRelation::CopyOf { skill: own.name.clone(), drift });
    }
    if matches!(body.origin, Origin::SkillsCli { .. }) {
        let hit = repo.declared.iter().find_map(|d| d.skills.iter().find(|s| answers_to(body, s)).map(|s| (d, s)));
        if let Some((d, skill)) = hit {
            return Some(JstackRelation::Declared { source: d.source.clone(), skill: skill.clone() });
        }
    }
    None
}

/// Pure. Relations for every body, the repo skills each present harness lacks, and the declared skills that are missing.
pub fn report(snapshot: &Snapshot, repo: &RepoState) -> JstackReport {
    let mut relations: Vec<(SkillId, JstackRelation)> =
        snapshot.bodies.values().filter_map(|b| relate(b, repo).map(|r| (b.id.clone(), r))).collect();
    relations.sort_by(|a, b| relation_name(&a.1).cmp(relation_name(&b.1)).then_with(|| a.0.cmp(&b.0)));

    // jstack links into the user-level skill folders, so a project-scope mount does not count.
    let mut installed: std::collections::HashSet<(HarnessId, &str)> = std::collections::HashSet::new();
    for (id, rel) in &relations {
        let (JstackRelation::Owned { skill, .. } | JstackRelation::CopyOf { skill, .. }) = rel else { continue };
        let mounts = snapshot.bodies.get(id).into_iter().flat_map(|b| &b.mounts).filter_map(|m| snapshot.mounts.get(m));
        for m in mounts.filter(|m| m.scope == Scope::User) {
            installed.insert((m.harness, skill.as_str()));
        }
    }

    let mut not_installed = Vec::new();
    for harness in snapshot.harnesses.iter().filter(|h| h.present) {
        for skill in repo.skills.iter().filter(|s| !repo.excluded.contains(&s.name)) {
            if installed.contains(&(harness.id, skill.name.as_str())) {
                continue;
            }
            let holder = snapshot
                .mounts
                .values()
                .filter(|m| m.harness == harness.id && !matches!(m.scope, Scope::Project { .. }))
                .filter_map(|m| snapshot.bodies.get(&m.body))
                .filter(|b| b.manifest.name == skill.name)
                .map(|b| &b.id)
                .min();
            let why = holder.map_or(Absence::Absent, |by| Absence::NameTaken { by: by.clone() });
            not_installed.push(NotInstalled { harness: harness.id, skill: skill.name.clone(), why });
        }
    }
    not_installed.sort_by(|a, b| a.harness.cmp(&b.harness).then_with(|| a.skill.cmp(&b.skill)));

    let provided: std::collections::HashSet<(&str, &str)> = relations
        .iter()
        .filter_map(|(_, r)| match r {
            JstackRelation::Declared { source, skill } => Some((source.as_str(), skill.as_str())),
            _ => None,
        })
        .collect();
    let declared_missing = repo
        .declared
        .iter()
        .flat_map(|d| d.skills.iter().map(move |s| (d, s)))
        .filter(|(d, s)| !provided.contains(&(d.source.as_str(), s.as_str())))
        .map(|(d, s)| DeclaredMissing { source: d.source.clone(), skill: s.clone() })
        .collect();

    JstackReport { relations, not_installed, declared_missing }
}

fn owned_drift(repo: &RepoState, skill: &str) -> Drift {
    let folder = Path::new("skills").join(skill);
    let files: Vec<PathBuf> = repo.dirty.iter().filter(|p| p.starts_with(&folder)).cloned().collect();
    if !files.is_empty() {
        return Drift::UncommittedEdits { files };
    }
    match &repo.upstream {
        Some(Upstream { ahead, behind, .. }) if *behind > 0 && *ahead > 0 => Drift::Diverged { ahead: *ahead, behind: *behind },
        Some(Upstream { behind, .. }) if *behind > 0 => Drift::Behind { commits: *behind },
        _ => Drift::InSync,
    }
}

/// The folder name is what the skills lock and the repo use. The frontmatter name is what harnesses load.
fn answers_to(body: &SkillBody, name: &str) -> bool {
    body.manifest.name == name || body.dir.as_path().file_name().is_some_and(|f| f == name)
}

fn relation_name(r: &JstackRelation) -> &str {
    match r {
        JstackRelation::Owned { skill, .. } | JstackRelation::CopyOf { skill, .. } | JstackRelation::Declared { skill, .. } => skill,
    }
}

// ---------------------------------------------------------------- commands

/// What jstack's own tooling can do for the user. skilltab only describes the command line. The
/// manage module shows it in a plan and runs it, and nothing in this module ever does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JstackAction {
    /// Install or re-link: `sync-skills.ps1`. It links every own skill into every harness, the way
    /// jstack decides, and it pulls the clone first.
    Sync,
    /// Update the clone only: `git pull --ff-only`. Owned skills change with it, since they are the clone's folders.
    Pull,
    /// One line of skill-sources.txt, as install-tools.ps1 runs it. The skills CLI updates a skill that is already installed.
    InstallDeclared { source: String, skills: Vec<String> },
}

pub struct JstackCommand {
    pub command: crate::harness::ExternalCommand,
    /// True when running it moves the clone to a new commit. sync-skills.ps1 does this on every
    /// run, even with -WhatIf, so a preview of it is not a preview of the working tree.
    pub updates_clone: bool,
}

impl RepoState {
    /// Pure. Whether sync-skills.ps1 exists in the clone is the planner's check, not this function's.
    pub fn command(&self, action: &JstackAction) -> JstackCommand {
        let root = self.root.clone();
        let text = |s: &str| s.to_owned();
        match action {
            JstackAction::Sync => JstackCommand {
                // The same line the session-start hooks use, plus -repo so a clone outside ~\jstack works.
                command: crate::harness::ExternalCommand {
                    program: text("powershell"),
                    args: ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]
                        .map(text)
                        .into_iter()
                        .chain([root.join("sync-skills.ps1").to_string_lossy().into_owned(), text("-repo"), root.to_string_lossy().into_owned()])
                        .collect(),
                    cwd: Some(root),
                },
                updates_clone: true,
            },
            JstackAction::Pull => JstackCommand {
                command: crate::harness::ExternalCommand {
                    program: text("git"),
                    args: ["pull", "--ff-only"].map(text).into(),
                    cwd: Some(root),
                },
                updates_clone: true,
            },
            JstackAction::InstallDeclared { source, skills } => JstackCommand {
                // install-tools.ps1: npx --yes skills add <source> -g -a claude-code codex -y --skill <skills...>
                // `npx` is a .cmd shim on Windows, which CreateProcess only finds with its extension.
                command: crate::harness::ExternalCommand {
                    program: text("npx.cmd"),
                    args: ["--yes", "skills", "add"]
                        .map(text)
                        .into_iter()
                        .chain([source.clone()])
                        .chain(["-g", "-a", "claude-code", "codex", "-y", "--skill"].map(text))
                        .chain(skills.iter().cloned())
                        .collect(),
                    cwd: Some(root),
                },
                updates_clone: false,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fsutil::strip_verbatim;
    use std::fs;
    use std::process::Command;

    /// A real temp repo, driven through the same `git` CLI the module uses.
    pub(super) struct TempRepo {
        _guard: tempfile::TempDir,
        pub root: PathBuf,
    }

    impl TempRepo {
        pub fn new() -> TempRepo {
            let guard = tempfile::tempdir().unwrap();
            let root = strip_verbatim(fs::canonicalize(guard.path()).unwrap()).join("jstack");
            fs::create_dir_all(&root).unwrap();
            let repo = TempRepo { _guard: guard, root };
            repo.git(&["init", "-q", "-b", "main"]);
            repo
        }

        pub fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "commit.gpgsign=false"])
                .args(["-c", "core.autocrlf=false"])
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        }

        pub fn write(&self, rel: &str, text: &str) {
            let path = self.root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }

        pub fn skill(&self, name: &str, body: &str) {
            self.write(&format!("skills/{name}/SKILL.md"), &format!("---\nname: {name}\ndescription: d\n---\n{body}\n"));
        }

        pub fn commit_all(&self, message: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-q", "-m", message]);
        }
    }

    #[test]
    fn load_reads_head_branch_skills_and_config_files() {
        let repo = TempRepo::new();
        repo.skill("alpha", "one");
        repo.skill("beta", "two");
        repo.write("skill-sources.txt", "\u{feff}# comment\nowner/pack one two\n\nhttps://x.example three\n");
        repo.write("exclude.txt", "# never mirrored\nsecret\n");
        repo.commit_all("init");

        let state = RepoState::load(&repo.root).unwrap();

        assert_eq!(state.head, repo.git(&["rev-parse", "HEAD"]));
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!(state.upstream, None);
        assert_eq!(state.remote, None);
        assert!(state.dirty.is_empty(), "{:?}", state.dirty);
        assert_eq!(state.skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["alpha", "beta"]);
        assert_eq!(
            state.declared,
            [
                DeclaredSource { source: "owner/pack".into(), skills: vec!["one".into(), "two".into()] },
                DeclaredSource { source: "https://x.example".into(), skills: vec!["three".into()] },
            ]
        );
        assert_eq!(state.excluded, ["secret"]);
    }

    #[test]
    fn dirty_lists_modified_and_untracked_files_under_skills_only() {
        let repo = TempRepo::new();
        repo.skill("alpha", "one");
        repo.write("README.md", "readme");
        repo.commit_all("init");

        repo.skill("alpha", "edited");
        repo.skill("fresh", "new");
        repo.write("README.md", "edited readme");

        let mut dirty: Vec<String> = RepoState::load(&repo.root).unwrap().dirty.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        dirty.sort();
        assert_eq!(dirty, ["skills/alpha/SKILL.md", "skills/fresh/SKILL.md"]);
    }

    #[test]
    fn load_reports_upstream_ahead_and_behind_and_remote() {
        let upstream = TempRepo::new();
        upstream.skill("alpha", "one");
        upstream.commit_all("init");

        let clone_root = upstream.root.parent().unwrap().join("clone");
        let out = Command::new("git").arg("clone").arg("-q").arg(&upstream.root).arg(&clone_root).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

        upstream.skill("alpha", "two");
        upstream.commit_all("upstream moves");
        fetch(&clone_root).unwrap();
        let state = RepoState::load(&clone_root).unwrap();

        let up = state.upstream.expect("clone tracks origin/main");
        assert_eq!((up.name.as_str(), up.ahead, up.behind), ("origin/main", 0, 1));
        assert!(state.remote.is_some());
        assert!(state.fetched_at.is_some());
    }

    #[test]
    fn missing_folder_and_plain_folder_are_typed_errors() {
        let repo = TempRepo::new();
        assert!(matches!(RepoState::load(&repo.root.join("nope")), Err(JstackError::NotFound(_))));
        let plain = repo.root.parent().unwrap().join("plain");
        fs::create_dir_all(&plain).unwrap();
        assert!(matches!(RepoState::load(&plain), Err(JstackError::NotAGitRepo(_))));
        // A fresh repo has no HEAD to report.
        assert!(matches!(RepoState::load(&repo.root), Err(JstackError::Git { .. })));
    }
}

#[cfg(test)]
mod relation_tests {
    use super::tests::TempRepo;
    use super::*;
    use crate::Env;
    fn snapshot(env: &Env) -> crate::Snapshot {
        crate::scan_disk(env, &[], 1)
    }
    use std::fs;
    use std::process::Command;

    /// A fake home with a real jstack clone and the folders each case needs. Harness folders are
    /// the real ones (`.claude/skills`, `.agents/skills`), so the snapshot comes from the real scanner.
    struct Home {
        repo: TempRepo,
        home: PathBuf,
    }

    impl Home {
        fn new() -> Home {
            let repo = TempRepo::new();
            for (name, body) in [("alpha", "one"), ("beta", "two"), ("gamma", "three"), ("delta", "four")] {
                repo.skill(name, body);
            }
            repo.write("skill-sources.txt", "owner/pack ext-installed ext-missing\n");
            repo.commit_all("init");
            let home = repo.root.parent().unwrap().to_path_buf();
            Home { repo, home }
        }

        fn folder(&self, rel: &str, name: &str, body: &str) -> PathBuf {
            let dir = self.home.join(rel);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\n{body}\n")).unwrap();
            dir
        }

        fn junction(&self, link: &str, target: &Path) {
            let link = self.home.join(link.replace('/', "\\"));
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            let out = Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(target.to_string_lossy().replace('/', "\\")).output().unwrap();
            assert!(out.status.success(), "mklink: {}", String::from_utf8_lossy(&out.stdout));
        }

        fn lock(&self, skill: &str, source: &str) {
            let dir = self.home.join(".agents");
            fs::create_dir_all(&dir).unwrap();
            let json = format!(r#"{{"version":3,"skills":{{"{skill}":{{"source":"{source}"}}}}}}"#);
            fs::write(dir.join(".skill-lock.json"), json).unwrap();
        }

        /// alpha: junction to the repo (Owned, Claude only). beta: edited copy in Codex. gamma: identical
        /// copy in Claude. delta: nowhere. ext-installed: a `skills` CLI install.
        fn standard() -> Home {
            let h = Home::new();
            h.junction(".claude/skills/alpha", &h.repo.root.join("skills").join("alpha"));
            h.folder(".agents/skills/beta", "beta", "changed");
            let copy = h.folder(".claude/skills/gamma", "x", "x");
            fs::copy(h.repo.root.join("skills/gamma/SKILL.md"), copy.join("SKILL.md")).unwrap();
            h.folder(".agents/skills/ext-installed", "ext-installed", "e");
            h.lock("ext-installed", "owner/pack");
            h
        }

        fn env(&self, root: &Path) -> Env {
            Env { home: self.home.clone(), projects: vec![], jstack_root: Some(root.to_path_buf()) }
        }

        fn report(&self) -> (Snapshot, RepoState, JstackReport) {
            let snap = snapshot(&self.env(&self.repo.root));
            let repo = RepoState::load(&self.repo.root).unwrap();
            let rep = report(&snap, &repo);
            (snap, repo, rep)
        }
    }

    fn relation_of<'a>(rep: &'a JstackReport, skill: &str) -> &'a JstackRelation {
        let found: Vec<_> = rep.relations.iter().map(|(_, r)| r).filter(|r| relation_name(r) == skill).collect();
        assert_eq!(found.len(), 1, "{skill}: {:?}", rep.relations);
        found[0]
    }

    #[test]
    fn owned_body_in_a_clean_repo_is_in_sync() {
        let (snap, _, rep) = Home::standard().report();
        assert!(snap.issues.is_empty(), "{:?}", snap.issues);
        assert_eq!(*relation_of(&rep, "alpha"), JstackRelation::Owned { skill: "alpha".into(), drift: Drift::InSync });
    }

    #[test]
    fn owned_body_with_uncommitted_edits_names_its_files_and_ignores_other_skills() {
        let h = Home::standard();
        h.repo.skill("alpha", "edited");
        h.repo.write("skills/alpha/notes.md", "new file");
        h.repo.skill("delta", "also edited");
        let (_, _, rep) = h.report();

        let JstackRelation::Owned { drift: Drift::UncommittedEdits { files }, .. } = relation_of(&rep, "alpha") else {
            panic!("expected edits: {:?}", relation_of(&rep, "alpha"));
        };
        let mut files: Vec<String> = files.iter().map(|f| f.to_string_lossy().replace('\\', "/")).collect();
        files.sort();
        assert_eq!(files, ["skills/alpha/SKILL.md", "skills/alpha/notes.md"]);
    }

    #[test]
    fn owned_body_is_behind_when_upstream_has_commits_and_diverged_when_both_sides_do() {
        let h = Home::standard();
        let clone = h.home.join("clone");
        let out = Command::new("git").arg("clone").arg("-q").arg(&h.repo.root).arg(&clone).output().unwrap();
        assert!(out.status.success());
        // Re-aim the junction at the clone's alpha so the body lives in the clone, then move origin on.
        fs::remove_dir(h.home.join(".claude/skills/alpha")).unwrap();
        h.junction(".claude/skills/alpha", &clone.join("skills").join("alpha"));
        h.repo.skill("beta", "origin moved");
        h.repo.commit_all("origin moves");
        fetch(&clone).unwrap();

        let snap = snapshot(&h.env(&clone));
        let alpha = |repo: &RepoState| {
            let rep = report(&snap, repo);
            relation_of(&rep, "alpha").clone()
        };
        let behind = RepoState::load(&clone).unwrap();
        assert_eq!(alpha(&behind), JstackRelation::Owned { skill: "alpha".into(), drift: Drift::Behind { commits: 1 } });

        fs::write(clone.join("local.txt"), "x").unwrap();
        let commit = |args: &[&str]| {
            let out = Command::new("git").arg("-C").arg(&clone).args(["-c", "user.name=t", "-c", "user.email=t@example.com"]).args(args).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        };
        commit(&["add", "-A"]);
        commit(&["commit", "-q", "-m", "local"]);
        let diverged = RepoState::load(&clone).unwrap();
        assert_eq!(alpha(&diverged), JstackRelation::Owned { skill: "alpha".into(), drift: Drift::Diverged { ahead: 1, behind: 1 } });
    }

    #[test]
    fn copy_with_identical_bytes_is_in_sync_and_edited_copy_differs() {
        let (_, repo, rep) = Home::standard().report();
        assert_eq!(*relation_of(&rep, "gamma"), JstackRelation::CopyOf { skill: "gamma".into(), drift: Drift::InSync });

        let own = repo.skills.iter().find(|s| s.name == "beta").unwrap();
        let JstackRelation::CopyOf { drift: Drift::ContentDiffers { installed, repo: repo_hash }, .. } = relation_of(&rep, "beta") else {
            panic!("expected a differing copy: {:?}", relation_of(&rep, "beta"));
        };
        assert_eq!(repo_hash, &own.content_hash);
        assert_ne!(installed, repo_hash);
    }

    #[test]
    fn a_copy_starts_differing_when_the_repo_skill_changes() {
        let h = Home::standard();
        h.repo.skill("gamma", "repo moved on");
        let (_, _, rep) = h.report();
        assert!(matches!(relation_of(&rep, "gamma"), JstackRelation::CopyOf { drift: Drift::ContentDiffers { .. }, .. }));
    }

    #[test]
    fn declared_skill_is_installed_only_when_the_skills_cli_installed_it_and_missing_otherwise() {
        let (_, _, rep) = Home::standard().report();
        assert_eq!(
            *relation_of(&rep, "ext-installed"),
            JstackRelation::Declared { source: "owner/pack".into(), skill: "ext-installed".into() }
        );
        assert_eq!(rep.declared_missing, [DeclaredMissing { source: "owner/pack".into(), skill: "ext-missing".into() }]);
    }

    #[test]
    fn declared_name_installed_by_something_other_than_the_skills_cli_stays_missing() {
        let h = Home::new();
        h.folder(".agents/skills/ext-installed", "ext-installed", "by hand");
        let (_, _, rep) = h.report();
        assert!(rep.relations.is_empty(), "{:?}", rep.relations);
        assert_eq!(rep.declared_missing.len(), 2);
    }

    #[test]
    fn repo_skills_missing_from_each_harness_are_listed_and_excluded_ones_are_not() {
        let h = Home::standard();
        h.repo.write("exclude.txt", "delta\n");
        h.repo.commit_all("exclude delta");
        let (snap, _, rep) = h.report();

        let missing = |harness: HarnessId| -> Vec<&str> {
            rep.not_installed.iter().filter(|n| n.harness == harness).map(|n| n.skill.as_str()).collect()
        };
        assert!(snap.harnesses.iter().all(|i| i.present));
        assert_eq!(missing(HarnessId::CLAUDE_CODE), ["beta"]);
        assert_eq!(missing(HarnessId::CODEX), ["alpha", "gamma"]);
        assert!(rep.not_installed.iter().all(|n| n.why == Absence::Absent));
    }

    #[test]
    fn a_project_copy_does_not_count_as_installed_for_jstack() {
        let h = Home::standard();
        let project = h.home.join("proj");
        h.folder("proj/.claude/skills/delta", "delta", "four");
        let env = Env { home: h.home.clone(), projects: vec![project], jstack_root: Some(h.repo.root.clone()) };
        let snap = snapshot(&env);
        let rep = report(&snap, &RepoState::load(&h.repo.root).unwrap());
        let delta = rep.not_installed.iter().find(|n| n.skill == "delta" && n.harness == HarnessId::CLAUDE_CODE).unwrap();
        assert_eq!(delta.why, Absence::Absent);
    }

    #[test]
    fn a_plugin_skill_with_the_same_name_blocks_the_link_and_is_named() {
        let h = Home::standard();
        let root = h.home.join("cache").join("p");
        h.folder("cache/p/skills/delta", "delta", "plugin delta");
        let claude = h.home.join(".claude");
        fs::create_dir_all(claude.join("plugins")).unwrap();
        let installed = format!(
            r#"{{"version":2,"plugins":{{"p@m":[{{"scope":"user","installPath":{}}}]}}}}"#,
            serde_json::to_string(&root).unwrap()
        );
        fs::write(claude.join("plugins/installed_plugins.json"), installed).unwrap();
        fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"p@m":true}}"#).unwrap();

        let (snap, _, rep) = h.report();
        let delta = rep.not_installed.iter().find(|n| n.skill == "delta" && n.harness == HarnessId::CLAUDE_CODE).unwrap();
        let Absence::NameTaken { by } = &delta.why else { panic!("{delta:?}") };
        assert_eq!(snap.bodies[by].manifest.name, "delta");
        assert!(rep.relations.iter().all(|(id, _)| id != by), "a plugin skill is not a copy of the repo's");
    }
}

#[cfg(test)]
mod command_tests {
    use super::tests::TempRepo;
    use super::*;

    fn state() -> (TempRepo, RepoState) {
        let repo = TempRepo::new();
        repo.skill("alpha", "one");
        repo.commit_all("init");
        let state = RepoState::load(&repo.root).unwrap();
        (repo, state)
    }

    #[test]
    fn sync_is_the_session_start_hook_line_aimed_at_this_clone_and_pulls_first() {
        let (repo, state) = state();
        let c = state.command(&JstackAction::Sync);
        let script = repo.root.join("sync-skills.ps1").to_string_lossy().into_owned();
        let root = repo.root.to_string_lossy().into_owned();
        assert_eq!(c.command.program, "powershell");
        assert_eq!(c.command.args, ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &script, "-repo", &root]);
        assert_eq!(c.command.cwd.as_deref(), Some(repo.root.as_path()));
        assert!(c.updates_clone);
    }

    #[test]
    fn update_is_a_fast_forward_only_pull_in_the_clone() {
        let (repo, state) = state();
        let c = state.command(&JstackAction::Pull);
        assert_eq!((c.command.program.as_str(), c.command.args.as_slice()), ("git", ["pull".to_owned(), "--ff-only".to_owned()].as_slice()));
        assert_eq!(c.command.cwd.as_deref(), Some(repo.root.as_path()));
        assert!(c.updates_clone);
    }

    #[test]
    fn declared_install_is_install_tools_line_with_each_skill_as_its_own_argument() {
        let (_, state) = state();
        let action = JstackAction::InstallDeclared { source: "owner/pack".into(), skills: vec!["one".into(), "two words".into()] };
        let c = state.command(&action);
        assert_eq!(c.command.program, "npx.cmd");
        assert_eq!(
            c.command.args,
            ["--yes", "skills", "add", "owner/pack", "-g", "-a", "claude-code", "codex", "-y", "--skill", "one", "two words"]
        );
        assert!(!c.updates_clone);
    }
}
