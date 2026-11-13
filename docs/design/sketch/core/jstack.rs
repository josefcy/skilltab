//! The link to the user's jstack repo. skilltab reads the repo and runs jstack's own scripts.
//! It does not re-implement jstack's linking policy (which harness gets which link,
//! exclude.txt, aliases.txt). jstack owns that policy, so skilltab never writes
//! ~/.jstack-links.txt and never edits the repo. Nothing is pushed.
//!
//! Repo layout relied on: skills/<name>/SKILL.md (own skills), skill-sources.txt
//! ("<source> <skill> <skill> ..."), exclude.txt, sync-skills.ps1.

use crate::harness::ExternalCommand;
use crate::model::*;
use std::path::{Path, PathBuf};

pub struct JstackRepo {
    pub root: PathBuf,
    pub remote: Option<String>,
    pub head: String,
    pub own: Vec<RepoSkill>,
    pub declared: Vec<DeclaredSource>,
    pub excluded: Vec<String>,
    /// From `git status --porcelain` and `rev-list --count HEAD..@{u}` (after an optional fetch).
    pub status: RepoStatus,
}

pub struct RepoSkill { pub name: String, pub dir: PathBuf, pub content_hash: ContentHash, pub dirty: Vec<PathBuf> }

pub struct DeclaredSource { pub source: String, pub skills: Vec<String> }

pub struct RepoStatus {
    pub behind: Option<u32>,
    pub ahead: u32,
    pub dirty: bool,
    pub fetched_at: Option<Timestamp>,
}

#[derive(Clone, Copy)]
pub enum FetchPolicy { UseLastFetch, Fetch }

/// Relation of one installed body to the repo. Derived; never stored.
pub enum JstackRelation {
    /// The body *is* the repo folder, reached through a junction. Its content cannot differ from the
    /// working tree, so drift here means "the working tree differs from HEAD" or "HEAD is behind origin".
    Owned { skill: String, drift: Drift },
    /// A separate folder with the same name as a repo skill. Drift is a content hash comparison.
    CopyOf { skill: String, drift: Drift },
    /// Listed in skill-sources.txt, installed by the skills CLI.
    Declared { source: String },
}

pub enum Drift {
    InSync,
    UncommittedEdits { files: Vec<PathBuf> },
    Behind { commits: u32 },
    Diverged,
}

/// Everything in the repo that is not installed anywhere. These are the "install" candidates.
pub struct NotInstalled { pub own: Vec<String>, pub declared: Vec<(String, String)> }

pub enum JstackAction {
    /// `pwsh -File sync-skills.ps1`. This links every own skill into every harness, the way jstack decides.
    Sync,
    /// `git pull --ff-only`, then Sync. This is "update" for own skills.
    PullAndSync,
    /// `npx skills add <source> -g -a claude-code codex -y --skill <skill>...`, which is jstack's own install line.
    InstallDeclared { source: String, skills: Vec<String> },
    /// Same command for declared skills that are already installed. The CLI updates them in place.
    UpdateDeclared { source: String, skills: Vec<String> },
}

impl JstackRepo {
    /// Runs `git` (installed) as a child process. `Fetch` touches the network and is only used when the user asks.
    pub fn load(root: &Path, fetch: FetchPolicy) -> Result<JstackRepo, JstackError> { unimplemented!() }

    /// Pure function.
    pub fn relate(&self, body: &SkillBody) -> Option<JstackRelation> { unimplemented!() }

    /// Pure function, over the bodies' relations.
    pub fn not_installed(&self, snapshot: &Snapshot) -> NotInstalled { unimplemented!() }

    /// Pure function. The planner shows this command in the plan before anything runs.
    pub fn command(&self, action: &JstackAction) -> ExternalCommand { unimplemented!() }
}

pub enum JstackError { NotFound(PathBuf), NotAGitRepo, Git(String) }
