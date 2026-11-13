//! One file per harness. A harness knows two things: where it looks for skills, and how its
//! session logs record a skill being used. It knows nothing about links, dedup, provenance,
//! deletion or storage. The scanner, provenance, the planner and the usage index own those.
//!
//! Adding a harness = one new private submodule + one line in REGISTRY + one HarnessId const.

use crate::model::{BundleId, HarnessId, InstallScope, InvocationKey, Ownership, ProjectRoot, Scope, Timestamp};
use crate::Env;
use std::path::{Path, PathBuf};

mod claude;
mod codex;

impl HarnessId {
    pub const CLAUDE_CODE: HarnessId = HarnessId("claude-code");
    pub const CODEX: HarnessId = HarnessId("codex");
}

static REGISTRY: &[&dyn Harness] = &[&claude::ClaudeCode, &codex::Codex];

pub fn registry() -> &'static [&'static dyn Harness] { REGISTRY }

#[cfg(test)]
#[test]
fn ids_are_unique() { /* the build fails if two registry entries share an id */ unimplemented!() }

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
    fn bundle_command(&self, bundle: &BundleRef, op: BundleOp) -> Option<ExternalCommand> { None }

    /// Project roots that live in the harness's own state, beyond transcript cwds.
    fn known_projects(&self, env: &Env) -> Vec<PathBuf> { Vec::new() }
}

// ---------------------------------------------------------------- discovery

#[derive(Default)]
pub struct Discovery {
    pub entries: Vec<EntryFact>,
    pub bundles: Vec<BundleFact>,
}

/// An unresolved fact: "harness H loads the folder at `entry` under this scope and name".
pub struct EntryFact {
    pub entry: PathBuf,
    pub scope: ScopeFact,
    pub naming: Naming,
    /// What the harness itself knows about who manages the entry. Provenance may refine it
    /// (to JstackSync or SkillsCli) when this says Unmanaged.
    pub owner_hint: Ownership,
    /// When two entries of this harness answer to the same name, the lower rank is the one
    /// the harness loads. Each harness has its own precedence rules, and only it knows them.
    pub rank: u8,
}

/// Like model::Scope, but a bundle is named by key because BundleIds are minted by the scanner.
pub enum ScopeFact { User, Project(ProjectRoot), Bundle { key: String } }

/// How the harness derives the invocation keys for an entry.
pub enum Naming {
    /// The manifest name ("tdd").
    Plain,
    /// "<prefix>:<manifest name>", as plugins and claude.ai-synced skills are named.
    Prefixed(String),
}

pub struct BundleFact {
    pub key: String,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub install_scope: InstallScope,
    pub enabled: bool,
    pub root: PathBuf,
}

impl Discovery {
    /// One EntryFact per child folder of `dir` that holds SKILL.md. Skips dot-folders
    /// (`.trash`, `.system` are handled by the caller on purpose). A missing `dir` is not an error.
    pub fn scan_skill_dir(&mut self, dir: &Path, scope: ScopeFact, owner: Ownership, naming: Naming, rank: u8) { unimplemented!() }
}

// ---------------------------------------------------------------- usage parsing

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
pub struct NoUsage;
impl SessionParser for NoUsage {
    fn feed(&mut self, _line: &[u8], _out: &mut Vec<RawEvent>) {}
    fn checkpoint(&self) -> ParserState { ParserState(String::new()) }
}

/// Opaque per-source parser state, stored next to the byte cursor.
pub struct ParserState(pub String);

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

#[derive(Clone, Copy)]
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

// ---------------------------------------------------------------- bundle ops

pub struct BundleRef<'a> { pub name: &'a str, pub marketplace: &'a str, pub install_scope: &'a InstallScope }

#[derive(Clone, Copy)]
pub enum BundleOp { Enable, Disable, Uninstall, Update }

/// A program and its arguments. Never a shell string, so values are never interpolated into a shell.
pub struct ExternalCommand { pub program: String, pub args: Vec<String>, pub cwd: Option<PathBuf> }
