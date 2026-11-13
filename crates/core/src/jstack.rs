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
