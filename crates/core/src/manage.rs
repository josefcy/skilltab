//! Every change to disk goes through here. `plan` is pure and holds all policy: which steps,
//! what else breaks, and what refuses. `apply` is a dumb executor that re-checks each step's
//! expectation against the live disk first. Bundle operations are plans too, with one `Run` step,
//! so the UI has one confirm flow.

mod setup;
mod shelf;
#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod win;

#[cfg(unix)]
use unix as sys;
#[cfg(windows)]
use win as sys;

use crate::harness::{registry, BundleOp, BundleRef, ExternalCommand};
use crate::model::*;
use crate::scan::{content_hash, still_link_to};
use crate::setup::ExportInput;
use sha1::{Digest, Sha1};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) use shelf::Shelf;
pub(crate) use sys::TRASH_NAME;

/// Tests make links through the same code `apply` uses. Production code cannot reach it.
#[cfg(test)]
pub(crate) fn create_link(entry: &Path, target: &Path) -> io::Result<()> {
    sys::create_link(entry, target)
}

// ---------------------------------------------------------------- requests and plans

#[derive(Clone, Debug)]
pub(crate) enum ManageAction {
    /// Take the skill away from one harness at one entry. A link goes to the shelf, so it can be put back.
    RemoveMount(MountId),
    /// Remove every mount's link, then send the real folder to the Recycle Bin or trash.
    DeleteBody(SkillId),
    /// `DeleteBody` for several skills in one plan. A skill that cannot be deleted is skipped with its reasons
    /// and does not block the others.
    DeleteBodies(Vec<SkillId>),
    /// Shelve one mount: remove its link, or move its real folder into the shelf.
    Disable(MountId),
    Restore(ShelfId),
    Bundle(BundleId, BundleOp),
    /// Write this machine's setup into the setup folder: `manifest.json` and the allowlisted config files.
    /// Holds what `setup::read_export` read, so planning stays pure.
    ExportSetup(Arc<ExportInput>),
}

pub(crate) struct Plan {
    pub revision: u64,
    pub action: ManageAction,
    pub steps: Vec<Step>,
    pub effects: Vec<Effect>,
    pub blockers: Vec<Blocker>,
    /// Only a bulk delete fills this. Each skill here contributes no steps.
    pub skipped: Vec<Skipped>,
    /// Plan lines that are neither a step nor a blocker: what is already in place, what needs the user's eye,
    /// and what only the user can do afterwards.
    pub notes: Vec<Note>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Note {
    pub kind: NoteKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoteKind {
    /// Already as wanted, so it has no step.
    Present,
    /// Left as it is, or left out, and the user should know.
    Attention,
    /// Something only the user can do after the plan runs. Export has none. Restore's sign-in lines are these.
    #[allow(dead_code)]
    Manual,
}

pub(crate) struct Skipped {
    pub skill: SkillId,
    pub blockers: Vec<Blocker>,
}

/// The only kinds of change skilltab makes. Each step carries what it expects to find on disk,
/// and `apply` refuses the whole plan when an unfinished step's expectation no longer holds.
#[derive(Clone, Debug)]
pub(crate) enum Step {
    /// Delete the link itself, never what it points to. `shelve` is written first when the link can be put back.
    RemoveLink { link: LinkPath, target: PathBuf, shelve: Option<ShelfEntry> },
    /// Only a `RealDir` can be recycled, so a link never reaches the Recycle Bin or trash.
    Recycle { dir: RealDir, hash: ContentHash },
    Stash { dir: RealDir, hash: ContentHash, shelve: ShelfEntry, stash: PathBuf },
    /// Creates `entry` as a link to `target`. `shelf` is the shelf entry it puts back, or None for a first link.
    Link { entry: PathBuf, target: PathBuf, shelf: Option<ShelfId> },
    Unstash { shelf: ShelfId, stash: PathBuf, to: PathBuf },
    Run { command: ExternalCommand },
    /// Writes `bytes` to `to` and creates its parent folders. `was` says what the planner saw there.
    /// Over `Was::Absent` the file is created with `create_new`, so a file that appears later is never replaced.
    /// Over `Was::Hash` it goes through a temp file in the same folder and a rename.
    Write { to: PathBuf, bytes: Arc<[u8]>, was: Was },
    /// Deletes one file, only while it still has the content the planner saw.
    Delete { path: PathBuf, was: ContentHash },
}

/// What a `Write` expects to find at its destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Was {
    Absent,
    Hash(ContentHash),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    HarnessLosesSkill { mount: MountId },
    HarnessGainsSkill { mount: MountId },
    /// Removing `via` also breaks a mount the user did not pick.
    AlsoBreaks { mount: MountId, via: PathBuf },
    BodyRecycled { dir: PathBuf },
}

/// Any blocker means apply refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Blocker {
    /// The owner puts the entry back, so a removal would silently revert. `name` is the entry's folder name.
    RecreatedBy { owner: Ownership, name: String },
    PartOfBundle { bundle: BundleId },
    /// The entry is the real folder, and these other mounts still reach it.
    SharedBody { dependents: Vec<MountId> },
    HarnessBuiltin { harness: HarnessId },
    Unsupported { why: String, remedy: String },
}

// ---------------------------------------------------------------- planning (pure)

pub(crate) fn plan(snap: &Snapshot, shelf: &Shelf, action: ManageAction) -> Plan {
    let mut p = Plan::new(snap.revision, action.clone());
    match &action {
        ManageAction::RemoveMount(m) => take_mount(snap, shelf, m, ShelfReason::Removed, &mut p),
        ManageAction::Disable(m) => take_mount(snap, shelf, m, ShelfReason::Disabled, &mut p),
        ManageAction::DeleteBody(b) => delete_body(snap, b, &mut p),
        ManageAction::DeleteBodies(ids) => delete_bodies(snap, ids, &mut p),
        ManageAction::Restore(id) => restore(snap, id, &mut p),
        ManageAction::Bundle(b, op) => bundle(snap, b, *op, &mut p),
        ManageAction::ExportSetup(input) => setup::plan_export(input, &mut p),
    }
    p
}

impl Plan {
    fn new(revision: u64, action: ManageAction) -> Plan {
        Plan { revision, action, steps: Vec::new(), effects: Vec::new(), blockers: Vec::new(), skipped: Vec::new(), notes: Vec::new() }
    }
    fn block(&mut self, b: Blocker) {
        if !self.blockers.contains(&b) {
            self.blockers.push(b);
        }
    }
    fn gone(&mut self, what: &str) {
        self.block(Blocker::Unsupported { why: format!("This {what} is no longer installed."), remedy: "Rescan and try again.".into() });
    }
}

fn take_mount(snap: &Snapshot, shelf: &Shelf, id: &MountId, reason: ShelfReason, p: &mut Plan) {
    let (Some(m), Some(body)) = (snap.mounts.get(id), snap.mounts.get(id).and_then(|m| snap.bodies.get(&m.body))) else {
        return p.gone("mount");
    };
    owner_blockers(m, p);
    p.effects.push(Effect::HarnessLosesSkill { mount: m.id.clone() });

    let shelf_id = shelf_id(m, snap.revision);
    let record = |held: Held| ShelfEntry {
        id: shelf_id.clone(),
        harness: m.harness,
        name: body.manifest.name.clone(),
        entry: m.entry.clone(),
        held,
        reason,
        at: snap.taken_at,
    };

    match m.hops.first() {
        Some(link) => {
            for other in snap.mounts_through(link.as_path()).iter().filter(|o| **o != m.id) {
                p.effects.push(Effect::AlsoBreaks { mount: other.clone(), via: link.as_path().to_path_buf() });
            }
            let target = next_hop(m, body);
            let shelve = Some(record(Held::Link { target: target.clone() }));
            p.steps.push(Step::RemoveLink { link: link.clone(), target, shelve });
        }
        None => {
            let dependents: Vec<MountId> = body.mounts.iter().filter(|o| **o != m.id).cloned().collect();
            if !dependents.is_empty() {
                p.block(Blocker::SharedBody { dependents });
            }
            let hash = body.content_hash.clone();
            match reason {
                ShelfReason::Removed => {
                    p.steps.push(Step::Recycle { dir: body.dir.clone(), hash });
                    p.effects.push(Effect::BodyRecycled { dir: body.dir.as_path().to_path_buf() });
                }
                ShelfReason::Disabled => {
                    let stash = shelf.stash_path(&shelf_id);
                    let shelve = record(Held::Folder { stash: stash.clone() });
                    p.steps.push(Step::Stash { dir: body.dir.clone(), hash, shelve, stash });
                }
            }
        }
    }
}

fn delete_body(snap: &Snapshot, id: &SkillId, p: &mut Plan) {
    let Some(body) = snap.bodies.get(id) else { return p.gone("skill") };
    let mut mounts: Vec<&Mount> = body.mounts.iter().filter_map(|m| snap.mounts.get(m)).collect();
    mounts.sort_by(|a, b| a.id.cmp(&b.id));

    match &body.origin {
        Origin::Plugin { bundle } => p.block(Blocker::PartOfBundle { bundle: bundle.clone() }),
        Origin::HarnessBuiltin { harness } => p.block(Blocker::HarnessBuiltin { harness: *harness }),
        Origin::ClaudeAiSync { .. } => p.block(Blocker::RecreatedBy { owner: Ownership::ClaudeAiSync, name: folder_name(body.dir.as_path()) }),
        Origin::SkillsCli { .. } | Origin::Unknown => {}
    }
    let mut removed: Vec<String> = Vec::new();
    for m in &mounts {
        owner_blockers(m, p);
        p.effects.push(Effect::HarnessLosesSkill { mount: m.id.clone() });
        if let Some(link) = m.hops.first()
            && !removed.contains(&norm_key(link.as_path()))
        {
            removed.push(norm_key(link.as_path()));
            p.steps.push(Step::RemoveLink { link: link.clone(), target: next_hop(m, body), shelve: None });
        }
    }
    p.steps.push(Step::Recycle { dir: body.dir.clone(), hash: body.content_hash.clone() });
    p.effects.push(Effect::BodyRecycled { dir: body.dir.as_path().to_path_buf() });
}

/// Each skill is planned alone first, so one that is blocked leaves the rest of the selection deletable.
fn delete_bodies(snap: &Snapshot, ids: &[SkillId], p: &mut Plan) {
    for id in ids {
        let mut one = Plan::new(snap.revision, ManageAction::DeleteBody(id.clone()));
        delete_body(snap, id, &mut one);
        if one.blockers.is_empty() {
            p.steps.append(&mut one.steps);
            p.effects.append(&mut one.effects);
        } else {
            p.skipped.push(Skipped { skill: id.clone(), blockers: one.blockers });
        }
    }
}

fn restore(snap: &Snapshot, id: &ShelfId, p: &mut Plan) {
    let Some(e) = snap.shelf.iter().find(|e| &e.id == id) else { return p.gone("shelf entry") };
    if !snap.mounts_through(&e.entry).is_empty() {
        p.block(Blocker::Unsupported {
            why: format!("Another skill is installed at {} now.", e.entry.display()),
            remedy: "Remove or disable that skill first.".into(),
        });
    }
    match &e.held {
        Held::Link { target } => {
            let known = snap.bodies.values().any(|b| norm_key(b.dir.as_path()) == norm_key(target)) || !snap.mounts_through(target).is_empty();
            if !known {
                p.block(Blocker::Unsupported {
                    why: format!("The folder this link pointed to, {}, is no longer installed.", target.display()),
                    remedy: "Reinstall the skill, then restore the link.".into(),
                });
            }
            p.steps.push(Step::Link { entry: e.entry.clone(), target: target.clone(), shelf: Some(e.id.clone()) });
        }
        Held::Folder { stash } => p.steps.push(Step::Unstash { shelf: e.id.clone(), stash: stash.clone(), to: e.entry.clone() }),
    }
}

fn bundle(snap: &Snapshot, id: &BundleId, op: BundleOp, p: &mut Plan) {
    let Some(b) = snap.bundles.get(id) else { return p.gone("plugin") };
    let mut members: Vec<&Mount> =
        snap.mounts.values().filter(|m| matches!(&m.scope, Scope::Bundle { bundle } if bundle == id)).collect();
    members.sort_by(|a, b| a.id.cmp(&b.id));

    if members.iter().any(|m| m.owner == Ownership::HarnessBuiltin) {
        p.block(Blocker::HarnessBuiltin { harness: b.harness });
    }
    let wants_enabled = op == BundleOp::Enable;
    if b.enabled == wants_enabled {
        return;
    }
    for m in &members {
        p.effects.push(if wants_enabled {
            Effect::HarnessGainsSkill { mount: m.id.clone() }
        } else {
            Effect::HarnessLosesSkill { mount: m.id.clone() }
        });
    }
    let reference = BundleRef { name: &b.name, marketplace: &b.marketplace, install_scope: &b.install_scope };
    let command = registry().iter().find(|h| h.id() == b.harness).and_then(|h| h.bundle_command(&reference, op));
    match command {
        Some(command) => p.steps.push(Step::Run { command }),
        None => {
            let label = registry().iter().find(|h| h.id() == b.harness).map_or("This harness", |h| h.label());
            p.block(Blocker::Unsupported {
                why: format!("{label} has no command skilltab can run to turn this plugin on or off."),
                remedy: format!("Change it in {label}'s own plugin settings."),
            });
        }
    }
}

/// Who would put the entry back, or who owns it instead of the user.
fn owner_blockers(m: &Mount, p: &mut Plan) {
    match &m.owner {
        Ownership::ClaudeAiSync => {
            p.block(Blocker::RecreatedBy { owner: m.owner.clone(), name: folder_name(&m.entry) })
        }
        Ownership::PluginManager { bundle } => p.block(Blocker::PartOfBundle { bundle: bundle.clone() }),
        Ownership::HarnessBuiltin => p.block(Blocker::HarnessBuiltin { harness: m.harness }),
        Ownership::SkillsCli | Ownership::Skilltab | Ownership::Unmanaged => {}
    }
    if let Scope::Bundle { bundle } = &m.scope {
        p.block(Blocker::PartOfBundle { bundle: bundle.clone() });
    }
}

/// What the entry link points at: the next link in the chain, or the folder itself.
fn next_hop(m: &Mount, body: &SkillBody) -> PathBuf {
    m.hops.get(1).map_or_else(|| body.dir.as_path().to_path_buf(), |h| h.as_path().to_path_buf())
}

fn folder_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn shelf_id(m: &Mount, revision: u64) -> ShelfId {
    let mut h = Sha1::new();
    h.update(m.harness.as_str().as_bytes());
    h.update([0]);
    h.update(norm_key(&m.entry).as_bytes());
    h.update(revision.to_le_bytes());
    ShelfId::new(h.finalize()[..6].iter().map(|b| format!("{b:02x}")).collect())
}

// ---------------------------------------------------------------- applying (impure)

/// The two operations a test must be able to observe without doing them for real.
pub(crate) trait Os: Send + Sync {
    fn recycle(&self, dir: &RealDir) -> io::Result<()>;
    /// Runs the command to completion and returns its output.
    fn run(&self, command: &ExternalCommand) -> io::Result<String>;
}

pub(crate) struct SystemOs;

impl Os for SystemOs {
    fn recycle(&self, dir: &RealDir) -> io::Result<()> {
        sys::recycle(dir)
    }

    fn run(&self, c: &ExternalCommand) -> io::Result<String> {
        let command = |program: &str| {
            let mut cmd = std::process::Command::new(program);
            cmd.args(&c.args).stdin(std::process::Stdio::null());
            if let Some(dir) = &c.cwd {
                cmd.current_dir(dir);
            }
            cmd
        };
        let spawned = command(&c.program).output();
        // npm installs CLIs as `<name>.cmd`, which Command does not find under the bare name.
        #[cfg(windows)]
        let spawned = match spawned {
            Err(e) if e.kind() == io::ErrorKind::NotFound => command(&format!("{}.cmd", c.program)).output(),
            other => other,
        };
        let out = spawned.map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => io::Error::new(e.kind(), format!("`{}` is not installed or not on PATH", c.program)),
            _ => e,
        })?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_owned();
        if !out.status.success() {
            return Err(io::Error::other(format!("{} exited with {}: {text}", c.display(), out.status)));
        }
        Ok(text)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StepResult {
    Done,
    /// Its end state already held.
    Skipped,
    Failed(String),
    /// An earlier step failed.
    NotRun,
}

#[derive(Debug)]
pub(crate) enum ApplyOutcome {
    Refused,
    /// The disk no longer matches the plan. Nothing ran.
    Stale { reasons: Vec<String> },
    Applied { results: Vec<StepResult>, output: Vec<String> },
}

/// Checks every unfinished step's expectation first, then runs the steps in order and stops at the
/// first failure. A step whose end state already holds is skipped, so applying twice, or after a
/// crash part way through, converges on the same disk.
pub(crate) fn apply(plan: &Plan, shelf: &mut Shelf, os: &dyn Os) -> ApplyOutcome {
    if !plan.blockers.is_empty() {
        return ApplyOutcome::Refused;
    }
    let reasons: Vec<String> = plan.steps.iter().filter(|s| !is_done(s)).filter_map(stale_reason).collect();
    if !reasons.is_empty() {
        return ApplyOutcome::Stale { reasons };
    }

    let mut results = Vec::new();
    let mut output = Vec::new();
    let mut failed = false;
    for step in &plan.steps {
        let result = if failed {
            StepResult::NotRun
        } else {
            match run_step(step, shelf, os, &mut output) {
                Ok(r) => r,
                Err(e) => {
                    failed = true;
                    StepResult::Failed(e.to_string())
                }
            }
        };
        results.push(result);
    }
    ApplyOutcome::Applied { results, output }
}

fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok()
}

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

fn is_done(step: &Step) -> bool {
    match step {
        Step::RemoveLink { link, .. } => !exists(link.as_path()),
        Step::Recycle { dir, .. } => !exists(dir.as_path()),
        Step::Stash { dir, stash, .. } => !exists(dir.as_path()) && exists(stash),
        Step::Link { entry, target, .. } => still_link_to(entry, target),
        Step::Unstash { stash, to, .. } => !exists(stash) && exists(to),
        Step::Run { .. } => false,
        Step::Write { to, bytes, .. } => std::fs::read(to).is_ok_and(|have| have == **bytes),
        Step::Delete { path, .. } => !exists(path),
    }
}

fn stale_reason(step: &Step) -> Option<String> {
    let changed = |dir: &RealDir, hash: &ContentHash| {
        if is_link(dir.as_path()) || !dir.as_path().is_dir() {
            Some(format!("{} is no longer a plain folder.", dir.as_path().display()))
        } else if &content_hash(dir) != hash {
            Some(format!("{} changed since the plan was made.", dir.as_path().display()))
        } else {
            None
        }
    };
    match step {
        Step::RemoveLink { link, target, .. } => (!still_link_to(link.as_path(), target))
            .then(|| format!("{} no longer points to {}.", link.as_path().display(), target.display())),
        Step::Recycle { dir, hash } | Step::Stash { dir, hash, .. } => changed(dir, hash),
        Step::Link { entry, target, .. } => {
            if exists(entry) {
                Some(format!("Something is at {} now.", entry.display()))
            } else if !target.exists() {
                Some(format!("{} is gone.", target.display()))
            } else {
                None
            }
        }
        Step::Unstash { stash, to, .. } => {
            if exists(to) {
                Some(format!("Something is at {} now.", to.display()))
            } else if !stash.is_dir() {
                Some(format!("The shelved folder {} is gone.", stash.display()))
            } else {
                None
            }
        }
        Step::Run { .. } => None,
        Step::Write { to, was: Was::Absent, .. } => exists(to).then(|| format!("Something is at {} now.", to.display())),
        Step::Write { to, was: Was::Hash(hash), .. } | Step::Delete { path: to, was: hash } => match std::fs::read(to) {
            Ok(have) if &ContentHash::of(&have) == hash => None,
            Ok(_) => Some(format!("{} changed since the plan was made.", to.display())),
            Err(_) => Some(format!("{} can no longer be read.", to.display())),
        },
    }
}

fn stamped(e: &ShelfEntry) -> ShelfEntry {
    ShelfEntry { at: chrono::Utc::now(), ..e.clone() }
}

fn run_step(step: &Step, shelf: &mut Shelf, os: &dyn Os, output: &mut Vec<String>) -> io::Result<StepResult> {
    match step {
        Step::Link { shelf: Some(id), .. } | Step::Unstash { shelf: id, .. } if is_done(step) => {
            shelf.forget(id)?;
            return Ok(StepResult::Skipped);
        }
        _ if is_done(step) => return Ok(StepResult::Skipped),
        _ => {}
    }
    match step {
        Step::RemoveLink { link, shelve, .. } => {
            if let Some(e) = shelve {
                shelf.record(stamped(e))?;
            }
            sys::remove_link(link)?;
        }
        Step::Recycle { dir, .. } => os.recycle(dir)?,
        Step::Stash { dir, shelve, stash, .. } => {
            shelf.record(stamped(shelve))?;
            if let Some(parent) = stash.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(dir.as_path(), stash).map_err(|e| moved_error(e, stash))?;
        }
        Step::Link { entry, target, shelf: id } => {
            if let Some(parent) = entry.parent() {
                std::fs::create_dir_all(parent)?;
            }
            sys::create_link(entry, target)?;
            if let Some(id) = id {
                shelf.forget(id)?;
            }
        }
        Step::Unstash { shelf: id, stash, to } => {
            std::fs::rename(stash, to).map_err(|e| moved_error(e, to))?;
            shelf.forget(id)?;
        }
        Step::Run { command } => output.push(os.run(command)?),
        Step::Write { to, bytes, was } => write_file(to, bytes, was)?,
        Step::Delete { path, .. } => std::fs::remove_file(path)?,
    }
    Ok(StepResult::Done)
}

fn write_file(to: &Path, bytes: &[u8], was: &Was) -> io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match was {
        Was::Absent => {
            let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(to)?;
            file.write_all(bytes).inspect_err(|_| {
                let _ = std::fs::remove_file(to);
            })
        }
        Was::Hash(_) => {
            let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let temp = to.with_file_name(format!(".{name}.skilltab-tmp"));
            std::fs::write(&temp, bytes)?;
            std::fs::rename(&temp, to).inspect_err(|_| {
                let _ = std::fs::remove_file(&temp);
            })
        }
    }
}

/// A rename cannot cross volumes. Copying and deleting instead would turn a cheap, atomic move into
/// a partial one, so the step fails and says why.
fn moved_error(e: io::Error, to: &Path) -> io::Error {
    if e.kind() == io::ErrorKind::CrossesDevices {
        io::Error::other(format!("{} is on another volume, and skilltab only moves folders within one volume.", to.display()))
    } else {
        e
    }
}

#[cfg(test)]
pub(crate) mod tests;
