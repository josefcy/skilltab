//! Every change to disk goes through here: plan (pure), then apply (impure, re-checked).
//! There is no other write path. Even jstack and bundle operations become plans with
//! a single `Run` step, so the UI has one confirm flow and one place shows what will happen.

use crate::harness::{registry, BundleOp, ExternalCommand};
use crate::jstack::{JstackAction, JstackRepo};
use crate::model::*;
use std::path::PathBuf;

// ---------------------------------------------------------------- requests

pub enum ManageAction {
    Remove { target: RemoveTarget },
    /// Reversible removal of one mount. It goes to the shelf.
    Disable { mount: MountId },
    /// Put a shelved mount back (it was disabled or removed).
    Restore { shelf: ShelfId },
    Bundle { bundle: BundleId, op: BundleOp },
    Jstack(JstackAction),
}

pub enum RemoveTarget {
    /// Take this skill away from one harness at one entry. The body stays if anything else uses it.
    Mount(MountId),
    /// Remove every mount of the body, then the body itself (to the Recycle Bin).
    Everywhere(SkillId),
    BrokenLink(MountId),
}

// ---------------------------------------------------------------- plans

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct PlanId(String);

pub struct Plan {
    pub id: PlanId,
    pub revision: u64,
    pub steps: Vec<Step>,
    pub effects: Vec<Effect>,
    pub blockers: Vec<Blocker>,
    /// Checked again just before the first step runs. If any fails, the plan is stale and nothing runs.
    preconditions: Vec<Precondition>,
}

/// The only kinds of filesystem change skilltab makes. Each one is idempotent:
/// if its end state already holds, it reports Skipped.
pub enum Step {
    /// RemoveDirectoryW on the reparse point. The target is untouched. The target is recorded on the shelf first.
    RemoveLink { link: LinkPath, shelf: ShelfId },
    /// IFileOperation with FOFX_RECYCLEONDELETE | FOF_NOCONFIRMATION. Never runs on a LinkPath (enforced by the type).
    Recycle { dir: RealDir },
    /// Rename the folder into the shelf stash (same volume as %LOCALAPPDATA% → a cheap rename, else copy and recycle).
    Stash { dir: RealDir, shelf: ShelfId },
    Relink { entry: PathBuf, target: PathBuf, shelf: ShelfId },
    Unstash { shelf: ShelfId, to: PathBuf },
    Run { command: ExternalCommand, why: String },
}

/// What the user loses or changes, derived from Snapshot::mounts_through.
pub enum Effect {
    HarnessLosesSkill { harness: HarnessId, mount: MountId },
    /// Removing this hop also breaks mounts the user did not pick (for example, the Codex real folder that a Claude junction points to).
    AlsoBreaks { mount: MountId, harness: HarnessId, via: PathBuf },
    BodyRecycled { dir: PathBuf },
    UsageBecomesGhost,
}

/// Any blocker means apply refuses. Blockers carry the remedy, so the UI can show the honest path.
pub enum Blocker {
    /// The owner will put it back. JstackSync → "add <name> to jstack/exclude.txt";
    /// ClaudeAiSync → "remove it on claude.ai"; SkillsCli → removal is fine but the lock
    /// still lists it, so `npx skills remove` is offered instead.
    RecreatedBy { owner: Ownership, remedy: String },
    /// The body lives in a plugin bundle. Use a Bundle action.
    PartOfBundle { bundle: BundleId },
    HarnessBuiltin,
    /// Recycling a body inside the jstack clone would delete the user's own source.
    InsideJstackRepo,
    /// The entry is the real folder, and other harnesses reach it through links.
    SharedBody { dependents: Vec<MountId> },
    /// No CLI for this bundle operation in this harness.
    Unsupported { why: String },
}

enum Precondition {
    LinkPointsTo { link: LinkPath, target: PathBuf },
    DirUnchanged { dir: RealDir, hash: ContentHash },
    Absent { path: PathBuf },
    SnapshotRevision(u64),
}

// ---------------------------------------------------------------- planning (pure)

/// Pure: (snapshot, action) → plan. All decisions are made here, and they are unit-testable
/// against hand-built snapshots with no filesystem.
///
/// Remove(Mount m):
///   m.hops empty (entry is the real dir):
///       no other mounts → Recycle(body.dir).
///       other mounts exist → Blocker::SharedBody. A real folder cannot leave one harness alone.
///       (Example: ~/.agents/skills/x is Codex's entry and also the target of Claude's junction.)
///       The remedy offered is Remove(Everywhere) or Disable on the other harness's mounts.
///   else → RemoveLink(hops[0]) + Effects for mounts_through(hops[0]) other than m.
///   owner.recreated_by() → Blocker::RecreatedBy.
/// Remove(Everywhere b):
///   For every mount: RemoveLink(hops[0]) (sorted deepest first, so no step works on a path an earlier step removed),
///   then Recycle(body.dir). Blockers come from any mount's owner, from bundle membership, from InsideJstackRepo.
/// Disable(m): like Remove(Mount) but Stash instead of Recycle, and the shelf reason is Disabled.
/// Bundle: harness.bundle_command(...) → Run, else Blocker::Unsupported.
/// Jstack(a): repo.command(a) → Run.
pub fn plan(snapshot: &Snapshot, jstack: Option<&JstackRepo>, action: ManageAction) -> Plan { unimplemented!() }

// ---------------------------------------------------------------- applying (impure)

pub enum StepResult { Done, Skipped, Failed(String) }

pub enum ApplyOutcome {
    /// The disk changed since planning. Nothing ran. The UI re-plans.
    Stale { failed: Vec<String> },
    Applied { results: Vec<StepResult>, command_output: Option<String> },
    Refused { blockers: usize },
}

/// Re-checks the preconditions, then runs the steps in order and stops at the first failure.
/// Every step is idempotent, so retrying a partly applied plan (after re-planning) is safe.
/// The shelf record is written (fsync) *before* the step that needs it, so a crash between the two
/// leaves a restorable record and never a lost link target.
pub fn apply(plan: &Plan, shelf: &mut Shelf) -> ApplyOutcome { unimplemented!() }

// ---------------------------------------------------------------- shelf

/// Sole owner of %LOCALAPPDATA%\skilltab\shelf.json and the shelf\ stash folders.
/// Writes are atomic: write to a temp file, then rename over the old one.
pub struct Shelf { path: PathBuf, entries: Vec<ShelfEntry> }

impl Shelf {
    pub fn open(path: PathBuf) -> std::io::Result<Shelf> { unimplemented!() }
    pub fn entries(&self) -> &[ShelfEntry] { unimplemented!() }
    fn record(&mut self, entry: ShelfEntry) -> std::io::Result<()> { unimplemented!() }
    fn forget(&mut self, id: &ShelfId) -> std::io::Result<()> { unimplemented!() }
}

impl Ownership {
    /// The owner that will recreate an entry after skilltab removes it, if any.
    pub fn recreated_by(&self) -> Option<&'static str> { unimplemented!() }
}

mod win {
    //! The only code that calls Win32 file APIs (windows-rs). Private, so callers cannot bypass the planner.
    use crate::model::{LinkPath, RealDir};
    pub(super) fn remove_link(link: &LinkPath) -> std::io::Result<()> { unimplemented!() }
    pub(super) fn recycle(dir: &RealDir) -> std::io::Result<()> { unimplemented!() }
    pub(super) fn create_junction(entry: &std::path::Path, target: &std::path::Path) -> std::io::Result<()> { unimplemented!() }
}
