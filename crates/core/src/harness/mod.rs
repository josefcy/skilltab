//! One file per harness. A harness knows two things: where it looks for skills, and how its
//! session logs record a skill being used. It knows nothing about links, dedup, provenance,
//! deletion or storage. The scanner, provenance, the planner and the usage index own those.
//!
//! Adding a harness = one new private submodule + one line in REGISTRY + one HarnessId const.

use crate::fsutil;
use crate::model::{
    norm_key, HarnessId, InstallScope, InvocationKey, Ownership, ProjectRoot, Timestamp,
};
use crate::Env;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

impl HarnessId {
    pub const CLAUDE_CODE: HarnessId = HarnessId("claude-code");
    pub const CODEX: HarnessId = HarnessId("codex");
}

mod claude;
mod codex;

static REGISTRY: &[&dyn Harness] = &[&claude::ClaudeCode, &codex::Codex];

pub fn registry() -> &'static [&'static dyn Harness] {
    REGISTRY
}

#[cfg(test)]
#[test]
fn ids_are_unique() {
    let mut seen = HashSet::new();
    for h in registry() {
        assert!(seen.insert(h.id()), "duplicate harness id {:?}", h.id());
    }
}

pub trait Harness: Sync {
    fn id(&self) -> HarnessId;
    fn label(&self) -> &'static str;

    /// Report every entry this harness would load. Entries do not need to be followed
    /// through links. The scanner resolves links and merges duplicates.
    fn discover(&self, env: &Env, projects: &[ProjectRoot]) -> Discovery;

    /// Files that may hold usage records. Called on every refresh, so it must be cheap (readdir only).
    fn usage_sources(&self, env: &Env) -> Vec<UsageSource>;

    /// A parser for one source file. `resume` is the state saved at the last stored offset,
    /// for formats where early lines (such as Codex `session_meta`) give context to later ones.
    fn session_parser(&self, resume: Option<&ParserState>) -> Box<dyn SessionParser>;

    /// The command line that performs a bundle operation through the harness's own tooling.
    /// None means this harness cannot do that operation, and the planner reports a blocker.
    fn bundle_command(&self, _bundle: &BundleRef, _op: BundleOp) -> Option<ExternalCommand> {
        None
    }

    /// Project roots that live in the harness's own state, beyond transcript cwds.
    fn known_projects(&self, _env: &Env) -> Vec<PathBuf> {
        Vec::new()
    }
}

// ---------------------------------------------------------------- discovery

#[derive(Default)]
pub struct Discovery {
    pub entries: Vec<EntryFact>,
    pub bundles: Vec<BundleFact>,
    /// Folders and record files the harness could not read. A missing one is not a problem.
    pub unreadable: Vec<(PathBuf, String)>,
}

/// An unresolved fact: "harness H loads the folder at `entry` under this scope and name".
pub struct EntryFact {
    pub entry: PathBuf,
    pub scope: ScopeFact,
    pub naming: Naming,
    /// What the harness itself knows about who manages the entry. Provenance may refine it
    /// (to JstackSync or SkillsCli) when this says Unmanaged. For a bundle entry the scanner
    /// sets `PluginManager`, because only it mints BundleIds, unless this says HarnessBuiltin.
    pub owner_hint: Ownership,
    /// When two entries of this harness answer to the same name, the lower rank is the one
    /// the harness loads. Each harness has its own precedence rules, and only it knows them.
    pub rank: u8,
}

/// Like model::Scope, but a bundle is named by key because BundleIds are minted by the scanner.
#[derive(Clone)]
pub enum ScopeFact {
    User,
    Project(ProjectRoot),
    Bundle { key: String },
}

/// How the harness derives the invocation keys for an entry.
#[derive(Clone)]
pub enum Naming {
    /// The manifest name ("tdd").
    Plain,
    /// "<prefix>:<manifest name>", as plugins and claude.ai-synced skills are named.
    Prefixed(String),
}

pub struct BundleFact {
    /// Unique within the harness. The scanner turns it into the BundleId.
    pub key: String,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub install_scope: InstallFact,
    pub enabled: bool,
    pub root: PathBuf,
}

/// Like model::InstallScope, but a project is a path because only the scanner mints ProjectRoots.
pub enum InstallFact {
    User,
    Project(PathBuf),
}

impl Discovery {
    /// One EntryFact per child of `dir` that is a link or holds SKILL.md. A link is reported even
    /// when its target is gone, so the scanner can surface it as a broken link. Skips dot-folders
    /// (`.trash`, `.system` are handled by the caller on purpose). A missing `dir` is not an error.
    pub fn scan_skill_dir(&mut self, dir: &Path, scope: ScopeFact, owner: Ownership, naming: Naming, rank: u8) {
        let read = match std::fs::read_dir(dir) {
            Ok(r) => r,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                self.unreadable.push((dir.to_path_buf(), e.to_string()));
                return;
            }
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for item in read {
            match item {
                Ok(i) => children.push(i.path()),
                Err(e) => self.unreadable.push((dir.to_path_buf(), e.to_string())),
            }
        }
        children.sort();
        for path in children {
            let hidden = path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            let Ok(meta) = std::fs::symlink_metadata(&path) else { continue };
            let is_skill = meta.file_type().is_symlink() || (meta.is_dir() && path.join("SKILL.md").is_file());
            if is_skill {
                self.push_entry(path, scope.clone(), owner.clone(), naming.clone(), rank);
            }
        }
    }

    pub fn push_entry(&mut self, entry: PathBuf, scope: ScopeFact, owner: Ownership, naming: Naming, rank: u8) {
        self.entries.push(EntryFact { entry, scope, naming, owner_hint: owner, rank });
    }

    /// The skills a plugin exposes: the folders its manifest lists, and the default `skills/`.
    /// A listed folder is either a parent of skill folders or one skill folder itself.
    pub fn scan_plugin_skills(&mut self, root: &Path, manifests: &[&str], scope: ScopeFact, owner: Ownership, naming: Naming, rank: u8) {
        let mut dirs: Vec<PathBuf> = vec![root.join("skills")];
        for rel in manifests {
            let path = root.join(rel);
            match fsutil::read_json(&path) {
                Ok(Some(manifest)) => {
                    match manifest.get("skills") {
                        Some(serde_json::Value::String(s)) => dirs.push(join_relative(root, s)),
                        Some(serde_json::Value::Array(a)) => {
                            dirs.extend(a.iter().filter_map(|v| v.as_str()).map(|s| join_relative(root, s)));
                        }
                        _ => {}
                    }
                    break;
                }
                Ok(None) => {}
                Err(e) => self.unreadable.push((path, e)),
            }
        }
        let mut seen = HashSet::new();
        for dir in dirs {
            if !seen.insert(norm_key(&dir)) {
                continue;
            }
            if dir.join("SKILL.md").is_file() {
                self.push_entry(dir, scope.clone(), owner.clone(), naming.clone(), rank);
            } else {
                self.scan_skill_dir(&dir, scope.clone(), owner.clone(), naming.clone(), rank);
            }
        }
    }
}

// ---------------------------------------------------------------- usage parsing

#[derive(Debug, Clone)]
pub struct UsageSource {
    pub path: PathBuf,
    /// Append-only logs can be resumed from a byte offset. Others are reparsed whenever they change.
    pub append_only: bool,
}

pub trait SessionParser {
    /// Feed one complete line (no trailing newline). Implementations must check a cheap byte
    /// prefilter (memchr for a marker) before any JSON parse. Almost every line is skipped.
    fn feed(&mut self, line: &[u8], out: &mut Vec<RawEvent>);
    fn checkpoint(&self) -> ParserState;
}

/// For a harness whose usage log format is not known yet. The harness still shows up in the inventory.
/// No shipped harness needs it today; it is the starting point for adding one.
#[allow(dead_code)]
pub struct NoUsage;
impl SessionParser for NoUsage {
    fn feed(&mut self, _line: &[u8], _out: &mut Vec<RawEvent>) {}
    fn checkpoint(&self) -> ParserState {
        ParserState(String::new())
    }
}

/// Appends every `*.jsonl` file directly inside `dir`. A missing folder yields nothing.
fn push_jsonl(out: &mut Vec<UsageSource>, dir: &Path) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for entry in read.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|t| t.is_file()) && path.extension().is_some_and(|e| e == "jsonl") {
            out.push(UsageSource { path, append_only: true });
        }
    }
}

fn parse_ts(s: &str) -> Option<Timestamp> {
    chrono::DateTime::parse_from_rfc3339(s).ok().map(|t| t.with_timezone(&chrono::Utc))
}

/// Opaque per-source parser state, stored next to the byte cursor.
#[derive(Debug, Clone)]
pub struct ParserState(pub String);

#[derive(Debug, Clone)]
pub struct RawEvent {
    pub key: InvocationKey,
    pub kind: InvocationKind,
    pub at: Timestamp,
    pub session: String,
    pub cwd: Option<PathBuf>,
    /// Unique per real event (e.g. tool_use id, message uuid). Resumed or copied sessions
    /// repeat lines across files. INSERT OR IGNORE on this key keeps ingest idempotent.
    pub dedup: String,
}

/// The discriminants are stored in `usage.db` and index `Tally::by_kind`, so they must not be reordered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvocationKind {
    /// The user typed /name (Claude `<command-name>`).
    Slash,
    /// The model called the Skill tool.
    Model,
    /// The user mentioned $name (Codex).
    Mention,
    /// The agent opened SKILL.md with a tool (Codex). This is the weakest signal, so the UI shows it apart.
    FileRead,
}

impl InvocationKind {
    pub const ALL: [InvocationKind; 4] =
        [InvocationKind::Slash, InvocationKind::Model, InvocationKind::Mention, InvocationKind::FileRead];

    pub fn from_index(i: i64) -> Option<InvocationKind> {
        usize::try_from(i).ok().and_then(|i| Self::ALL.get(i).copied())
    }

    pub fn label(self) -> &'static str {
        match self {
            InvocationKind::Slash => "slash",
            InvocationKind::Model => "model",
            InvocationKind::Mention => "mention",
            InvocationKind::FileRead => "file-read",
        }
    }
}

// ---------------------------------------------------------------- bundle ops

pub struct BundleRef<'a> {
    pub name: &'a str,
    pub marketplace: &'a str,
    pub install_scope: &'a InstallScope,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleOp {
    Enable,
    Disable,
}

/// A program and its arguments. Never a shell string, so values are never interpolated into a shell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalCommand {
    pub program: String,
    pub args: Vec<String>,
    /// Working directory. jstack's scripts and `git pull` must run inside the clone.
    pub cwd: Option<PathBuf>,
}

impl ExternalCommand {
    /// The command as a user would type it, for the plan dialog.
    pub fn display(&self) -> String {
        std::iter::once(self.program.as_str()).chain(self.args.iter().map(String::as_str)).collect::<Vec<_>>().join(" ")
    }
}

/// Manifests write paths like "./skills/". Dropping the `.` keeps one spelling per folder.
fn join_relative(root: &Path, rel: &str) -> PathBuf {
    rel.split(['/', '\\']).filter(|c| !c.is_empty() && *c != ".").fold(root.to_path_buf(), |p, c| p.join(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_relative_paths_collapse_to_one_spelling() {
        let root = Path::new(r"C:\p");
        assert_eq!(join_relative(root, "./skills/"), root.join("skills"));
        assert_eq!(join_relative(root, r".\skills\a"), root.join("skills").join("a"));
    }
}
