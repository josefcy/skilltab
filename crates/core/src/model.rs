//! Domain model. A `Snapshot` is a pure function of the filesystem at scan time.
//! Nothing in it is persisted. Usage counts live in `usage`, and they are joined
//! to the snapshot when read.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- ids

/// Harness id string such as "claude-code" or "codex". The only constructors are the
/// associated consts declared beside the registry, and `parse` only accepts registered ids.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct HarnessId(pub(crate) &'static str);
impl HarnessId {
    pub fn as_str(self) -> &'static str {
        self.0
    }
    pub fn parse(s: &str) -> Option<HarnessId> {
        crate::harness::registry().iter().map(|h| h.id()).find(|id| id.0 == s)
    }
}

/// Identity of a physical skill folder: (volume serial, file index) from
/// GetFileInformationByHandle. It survives renames within a volume and a different spelling
/// of the same path. Rebuilt on every scan, so a moved folder simply gets a new id.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct SkillId(String);

/// Mount id = hash(harness, entry path). Stable across scans while the entry exists.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct MountId(String);

/// "<harness>:<plugin>@<marketplace>". Version is a field, not identity, so an update keeps the id.
#[derive(Clone, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct BundleId(String);

impl SkillId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn mint(s: String) -> SkillId {
        SkillId(s)
    }
}
impl MountId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn mint(s: String) -> MountId {
        MountId(s)
    }
}
impl BundleId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn mint(s: String) -> BundleId {
        BundleId(s)
    }
}

// ---------------------------------------------------------------- verified paths
// These two newtypes are what make delete safe. The planner can only emit RemoveLink for
// a LinkPath and Recycle for a RealDir, and only the scanner constructs either (after an
// lstat). So recycling *through* a junction (which would empty the target that every other
// harness uses) does not type-check. A test in `scan` fails the build if another module
// calls `mint`.

/// A path whose final component is a reparse point (junction or symlink) at scan time.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct LinkPath(PathBuf);

/// A canonical directory that is not a reparse point and contains SKILL.md at scan time.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct RealDir(PathBuf);

impl LinkPath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub(crate) fn mint(p: PathBuf) -> LinkPath {
        LinkPath(p)
    }
}
impl RealDir {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub(crate) fn mint(p: PathBuf) -> RealDir {
        RealDir(p)
    }
}

/// A project root that skilltab knows about, either from transcript `cwd` values or from settings.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ProjectRoot(PathBuf);
impl ProjectRoot {
    pub fn path(&self) -> &Path {
        &self.0
    }
    pub(crate) fn mint(p: PathBuf) -> ProjectRoot {
        ProjectRoot(p)
    }
}

/// One spelling per folder, so every path lookup goes through this form. NTFS compares paths
/// case-insensitively, so on Windows the key is case-folded, separator-normalized and
/// verbatim-prefix-stripped.
#[cfg(windows)]
pub(crate) fn norm_key(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    let s = s.strip_prefix("\\\\?\\UNC\\").map(|r| format!("\\\\{r}")).unwrap_or(s);
    let s = s.strip_prefix("\\\\?\\").map(str::to_owned).unwrap_or(s);
    let s = s.strip_prefix("\\??\\").map(str::to_owned).unwrap_or(s);
    let trimmed = s.trim_end_matches('\\');
    let s = if trimmed.len() == 2 && trimmed.ends_with(':') { s } else { trimmed.to_owned() };
    s.to_lowercase()
}

/// Linux and macOS volumes can be case sensitive, and folding could merge two real folders, so the
/// spelling stays exact and only trailing separators go.
#[cfg(unix)]
pub(crate) fn norm_key(p: &Path) -> String {
    let s = p.to_string_lossy();
    match s.trim_end_matches('/') {
        "" => s.into_owned(),
        trimmed => trimmed.to_owned(),
    }
}

// ---------------------------------------------------------------- the skill body

#[derive(Debug, Clone)]
pub struct SkillBody {
    pub id: SkillId,
    pub dir: RealDir,
    pub manifest: Manifest,
    /// Tree hash over the folder's files (path + bytes), sorted. Used to spot two bodies that are
    /// byte-identical copies, and to check a folder is unchanged before it is moved or recycled.
    pub content_hash: ContentHash,
    pub origin: Origin,
    /// Not empty: a body exists in the snapshot only because some harness mounts it.
    pub mounts: Vec<MountId>,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ContentHash(pub [u8; 20]);

#[derive(Debug, Clone)]
pub struct Manifest {
    /// The frontmatter `name`, or the folder name when that is missing.
    pub name: String,
    pub description: Option<String>,
    pub disable_model_invocation: bool,
    pub issues: Vec<ManifestIssue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestIssue {
    NoFrontmatter,
    MissingName,
    NameDiffersFromFolder { folder: String },
    MissingDescription,
}

/// Where the folder's contents came from. One answer per body, decided by provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// Installed by the `skills` CLI. Facts copied from ~/.agents/.skill-lock.json.
    SkillsCli { source: String, source_url: Option<String>, git_ref: Option<String>, lock_hash: String },
    Plugin { bundle: BundleId },
    ClaudeAiSync { skill_id: String },
    HarnessBuiltin { harness: HarnessId },
    Unknown,
}

// ---------------------------------------------------------------- mounts

/// One place where one harness sees one body.
#[derive(Debug, Clone)]
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    User,
    Project { root: ProjectRoot },
    Bundle { bundle: BundleId },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ownership {
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
pub struct Bundle {
    pub id: BundleId,
    pub harness: HarnessId,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub install_scope: InstallScope,
    pub enabled: bool,
    /// A plugin root holds a manifest and a skills folder, not a SKILL.md, so it cannot be a `RealDir`.
    pub root: PathBuf,
    pub members: Vec<SkillId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallScope {
    User,
    Project { root: ProjectRoot },
}

// ---------------------------------------------------------------- shelf (disabled and removed links)

/// Minted by the planner from the entry and the revision, so re-planning the same action gives the same id.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ShelfId(String);

impl ShelfId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn new(s: String) -> ShelfId {
        ShelfId(s)
    }
}

/// A mount that skilltab took away and can put back. Shelved items show up in the
/// inventory as disabled rows, so "disabled" has one source of truth: the shelf.
#[derive(Debug, Clone)]
pub struct ShelfEntry {
    pub id: ShelfId,
    pub harness: HarnessId,
    /// The skill's manifest name when it was shelved, so the row can be shown without the folder.
    pub name: String,
    pub entry: PathBuf,
    pub held: Held,
    pub reason: ShelfReason,
    pub at: Timestamp,
}

#[derive(Debug, Clone)]
pub enum Held {
    /// The link was removed. Restoring it recreates a junction to `target`.
    Link { target: PathBuf },
    /// A real folder moved under %LOCALAPPDATA%\skilltab\shelf\<id>.
    Folder { stash: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShelfReason {
    Disabled,
    Removed,
}

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
    /// Derived from `mounts` in `Snapshot::new` and never mutated afterwards. These are the access
    /// paths for the planner (who depends on this path?) and for usage resolution (which
    /// body answers to this key?).
    index: SnapshotIndex,
}

struct SnapshotIndex {
    through_path: HashMap<String, Vec<MountId>>,
    by_key: HashMap<(HarnessId, InvocationKey), SkillId>,
}

pub(crate) struct SnapshotParts {
    pub revision: u64,
    pub taken_at: Timestamp,
    pub harnesses: Vec<HarnessInfo>,
    pub bodies: HashMap<SkillId, SkillBody>,
    pub mounts: HashMap<MountId, Mount>,
    pub bundles: HashMap<BundleId, Bundle>,
    pub shelf: Vec<ShelfEntry>,
    pub issues: Vec<ScanIssue>,
}

impl Snapshot {
    pub(crate) fn new(p: SnapshotParts) -> Snapshot {
        let index = SnapshotIndex::build(&p.mounts);
        Snapshot {
            revision: p.revision,
            taken_at: p.taken_at,
            harnesses: p.harnesses,
            bodies: p.bodies,
            mounts: p.mounts,
            bundles: p.bundles,
            shelf: p.shelf,
            issues: p.issues,
            index,
        }
    }

    /// Mounts whose `entry` or any hop is `path`. Removing `path` breaks exactly these.
    pub fn mounts_through(&self, path: &Path) -> &[MountId] {
        self.index.through_path.get(&norm_key(path)).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The body a harness would load for this key. Shadowed mounts are skipped. A mount of a
    /// disabled bundle still answers, so its history stays attached instead of turning into a ghost.
    pub fn resolve(&self, harness: HarnessId, key: &InvocationKey) -> Option<&SkillId> {
        self.index.by_key.get(&(harness, key.clone()))
    }
}

impl SnapshotIndex {
    fn build(mounts: &HashMap<MountId, Mount>) -> SnapshotIndex {
        let mut ordered: Vec<&Mount> = mounts.values().collect();
        ordered.sort_by(|a, b| a.id.cmp(&b.id));

        let mut through_path: HashMap<String, Vec<MountId>> = HashMap::new();
        for m in &ordered {
            let paths = std::iter::once(m.entry.as_path()).chain(m.hops.iter().map(LinkPath::as_path));
            for p in paths {
                let ids = through_path.entry(norm_key(p)).or_default();
                if !ids.contains(&m.id) {
                    ids.push(m.id.clone());
                }
            }
        }

        let mut by_key: HashMap<(HarnessId, InvocationKey), SkillId> = HashMap::new();
        for pass_active in [true, false] {
            for m in &ordered {
                let take = match m.state {
                    MountState::Active => pass_active,
                    MountState::BundleDisabled => !pass_active,
                    MountState::Shadowed { .. } => false,
                };
                if take {
                    for k in &m.keys {
                        by_key.entry((m.harness, k.clone())).or_insert_with(|| m.body.clone());
                    }
                }
            }
        }
        SnapshotIndex { through_path, by_key }
    }
}

#[derive(Debug, Clone)]
pub struct HarnessInfo {
    pub id: HarnessId,
    pub label: &'static str,
    pub present: bool,
}

#[derive(Debug, Clone)]
pub enum ScanIssue {
    /// Removable through `RemoveTarget::BrokenLink(id)`. Shown as its own row in the inventory.
    BrokenLink { id: MountId, link: LinkPath, target: PathBuf, harness: HarnessId },
    Unreadable { path: PathBuf, error: String },
    LinkCycle { entry: PathBuf },
}

pub type Timestamp = chrono::DateTime<chrono::Utc>;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn norm_key_ignores_case_separators_and_verbatim_prefix() {
        let a = norm_key(Path::new(r"\\?\C:\Users\Josef\.agents\skills\X\"));
        let b = norm_key(Path::new("c:/users/josef/.agents/skills/x"));
        assert_eq!(a, b);
        assert_eq!(norm_key(Path::new(r"C:\")), "c:\\");
    }

    #[cfg(unix)]
    #[test]
    fn norm_key_keeps_case_and_trims_trailing_separators() {
        assert_eq!(norm_key(Path::new("/home/u/.agents/skills/x/")), "/home/u/.agents/skills/x");
        assert_ne!(norm_key(Path::new("/home/u/Foo")), norm_key(Path::new("/home/u/foo")));
        assert_eq!(norm_key(Path::new("/")), "/");
        assert_eq!(norm_key(Path::new(r"/a\b")), r"/a\b");
    }
}

/// Usage of one (harness, key) pair, aggregated by `usage` and joined to mounts by `view`.
/// `by_kind` is indexed by `harness::InvocationKind as usize`.
#[derive(Default, Clone, Copy, Debug)]
pub struct Tally {
    pub total: u32,
    pub last_30d: u32,
    pub last_used: Option<Timestamp>,
    pub by_kind: [u32; 4],
}

/// Earliest transcript time skilltab has for a harness. "Unused" is only claimed after this.
pub type TrackingSince = std::collections::HashMap<HarnessId, Timestamp>;
