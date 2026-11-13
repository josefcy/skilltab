//! Domain model. A `Snapshot` is a pure function of the filesystem at scan time.
//! Nothing in it is persisted. Usage counts live in `usage`, and they are joined
//! to the snapshot when read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- ids

/// Harness id string such as "claude-code" or "codex". The only constructors are the
/// associated consts declared beside the registry, and `parse` only accepts registered ids.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct HarnessId(pub(crate) &'static str);
impl HarnessId {
    pub fn as_str(self) -> &'static str { self.0 }
    pub fn parse(s: &str) -> Option<HarnessId> { unimplemented!() } // looks up crate::harness::registry()
}

/// Identity of a physical skill folder: a hash of (volume serial, file index) from
/// GetFileInformationByHandle. It survives renames within a volume and a different spelling
/// of the same path. Rebuilt on every scan, so a moved folder simply gets a new id.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SkillId(String);

/// Mount id = hash(harness, entry path). Stable across scans while the entry exists.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct MountId(String);

/// "<harness>:<plugin>@<marketplace>". Version is a field, not identity, so an update keeps the id.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BundleId(String);

// ---------------------------------------------------------------- verified paths
// These two newtypes are what make delete safe. The planner can only emit RemoveLink for
// a LinkPath and Recycle for a RealDir, and only the scanner constructs either (after an
// lstat). So recycling *through* a junction (which would empty the target that every other
// harness uses) does not type-check.

/// A path whose final component is a reparse point (junction or symlink) at scan time.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LinkPath(PathBuf);

/// A canonical directory that is not a reparse point and contains SKILL.md at scan time.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RealDir(PathBuf);

impl LinkPath { pub fn as_path(&self) -> &Path { &self.0 } }
impl RealDir { pub fn as_path(&self) -> &Path { &self.0 } }

/// A project root that skilltab knows about, either from transcript `cwd` values or from settings.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ProjectRoot(PathBuf);
impl ProjectRoot { pub fn path(&self) -> &Path { &self.0 } }

// ---------------------------------------------------------------- the skill body

pub struct SkillBody {
    pub id: SkillId,
    pub dir: RealDir,
    pub manifest: Manifest,
    /// Tree hash over the folder's files (path + bytes), sorted. Used for jstack drift and
    /// to spot two bodies that are byte-identical copies.
    pub content_hash: ContentHash,
    pub origin: Origin,
    /// Not empty: a body exists in the snapshot only because some harness mounts it.
    pub mounts: Vec<MountId>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ContentHash(pub [u8; 20]);

pub struct Manifest {
    /// The frontmatter `name`, or the folder name when that is missing.
    pub name: String,
    pub description: Option<String>,
    pub disable_model_invocation: bool,
    pub issues: Vec<ManifestIssue>,
}

pub enum ManifestIssue { NoFrontmatter, MissingName, NameDiffersFromFolder { folder: String }, MissingDescription }

/// Where the folder's contents came from. One answer per body, decided by provenance.
pub enum Origin {
    /// The real folder sits inside the jstack clone (`jstack/skills/<name>`).
    JstackRepo { skill: String },
    /// Installed by the `skills` CLI. Facts copied from ~/.agents/.skill-lock.json.
    SkillsCli { source: String, source_url: Option<String>, git_ref: Option<String>, lock_hash: String },
    Plugin { bundle: BundleId },
    ClaudeAiSync { skill_id: String },
    HarnessBuiltin { harness: HarnessId },
    Unknown,
}

// ---------------------------------------------------------------- mounts

/// One place where one harness sees one body.
pub struct Mount {
    pub id: MountId,
    pub harness: HarnessId,
    pub scope: Scope,
    /// The path the harness reads. It may be a link, or it may be the real folder itself.
    pub entry: PathBuf,
    /// Every hop from `entry` to `body.dir`. `hops[0] == entry`. Empty when entry is the real folder.
    /// Hops are where a removal's side effects come from: removing any hop breaks this mount.
    pub hops: Vec<LinkPath>,
    pub body: SkillId,
    /// The harness-native names this mount answers to in usage logs.
    pub keys: Vec<InvocationKey>,
    /// Who created `entry`. This decides whether removing it lasts.
    pub owner: Ownership,
    pub state: MountState,
}

pub enum Scope {
    User,
    Project { root: ProjectRoot },
    Bundle { bundle: BundleId },
}

pub enum Ownership {
    /// Listed in ~/.jstack-links.txt. jstack's sync recreates it at the next session start.
    JstackSync,
    /// Made by the `skills` CLI (the lock file names it).
    SkillsCli,
    PluginManager { bundle: BundleId },
    /// claude.ai sync writes ~/.claude/skills/synced and will write it again.
    ClaudeAiSync,
    HarnessBuiltin,
    /// skilltab restored it from its shelf.
    Skilltab,
    Unmanaged,
}

pub enum MountState {
    Active,
    /// Another mount in the same harness answers to the same invocation key and wins.
    Shadowed { by: MountId },
    /// The bundle is installed but turned off in the harness config.
    BundleDisabled,
}

/// How a harness names a skill in its usage logs. Matching against these happens when read.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum InvocationKey {
    /// "tdd", "pstack:architect", "anthropic-skills:docs". Compare case-sensitively, as the harness does.
    Name(String),
    /// Canonical SKILL.md path that a harness opened (Codex reads skills as files).
    SkillFile(PathBuf),
}

// ---------------------------------------------------------------- bundles

pub struct Bundle {
    pub id: BundleId,
    pub harness: HarnessId,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub install_scope: InstallScope,
    pub enabled: bool,
    pub root: RealDir,
    pub members: Vec<SkillId>,
}

pub enum InstallScope { User, Project { root: ProjectRoot } }

// ---------------------------------------------------------------- shelf (disabled and removed links)

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShelfId(String);

/// A mount that skilltab took away and can put back. Shelved items show up in the
/// inventory as disabled rows, so "disabled" has one source of truth: the shelf.
pub struct ShelfEntry {
    pub id: ShelfId,
    pub harness: HarnessId,
    pub entry: PathBuf,
    pub held: Held,
    pub reason: ShelfReason,
    pub at: Timestamp,
}

pub enum Held {
    /// The link was removed. Restoring it recreates a junction to `target`.
    Link { target: PathBuf },
    /// A real folder moved under %LOCALAPPDATA%\skilltab\shelf\<id>.
    Folder { stash: PathBuf },
}

pub enum ShelfReason { Disabled, Removed }

// ---------------------------------------------------------------- snapshot

pub struct Snapshot {
    pub revision: u64,
    pub taken_at: Timestamp,
    pub harnesses: Vec<HarnessInfo>,
    pub bodies: HashMap<SkillId, SkillBody>,
    pub mounts: HashMap<MountId, Mount>,
    pub bundles: HashMap<BundleId, Bundle>,
    pub shelf: Vec<ShelfEntry>,
    pub issues: Vec<ScanIssue>,
    /// Built by `scan` from `mounts` and never mutated afterwards. These are the access
    /// paths for the planner (who depends on this path?) and for usage resolution (which
    /// body answers to this key?).
    index: SnapshotIndex,
}

struct SnapshotIndex {
    through_path: HashMap<PathBuf, Vec<MountId>>,
    by_key: HashMap<(HarnessId, InvocationKey), SkillId>,
}

impl Snapshot {
    /// Mounts whose `entry` or any hop is `path`. Removing `path` breaks exactly these.
    pub fn mounts_through(&self, path: &Path) -> &[MountId] { unimplemented!() }
    /// The body a harness would load for this key. Shadowed mounts are skipped.
    pub fn resolve(&self, harness: HarnessId, key: &InvocationKey) -> Option<&SkillId> { unimplemented!() }
}

pub struct HarnessInfo { pub id: HarnessId, pub label: &'static str, pub present: bool }

pub enum ScanIssue {
    /// Removable through `RemoveTarget::BrokenLink(id)`. Shown as its own row in the inventory.
    BrokenLink { id: MountId, link: LinkPath, target: PathBuf, harness: HarnessId },
    Unreadable { path: PathBuf, error: String },
    LinkCycle { entry: PathBuf },
}

pub type Timestamp = chrono::DateTime<chrono::Utc>;
