//! Every change to disk goes through here. `plan` is pure and holds all policy: which steps,
//! what else breaks, and what refuses. `apply` is a dumb executor that re-checks each step's
//! expectation against the live disk first. Bundle operations are plans too, with one `Run` step,
//! so the UI has one confirm flow.

mod shelf;
mod win;

use crate::harness::{registry, BundleOp, BundleRef, ExternalCommand};
use crate::jstack::{JstackAction, JstackCommand, RepoState};
use crate::model::*;
use crate::scan::{content_hash, still_link_to};
use sha1::{Digest, Sha1};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) use shelf::Shelf;

// ---------------------------------------------------------------- requests and plans

#[derive(Clone, Debug)]
pub(crate) enum ManageAction {
    /// Take the skill away from one harness at one entry. A link goes to the shelf, so it can be put back.
    RemoveMount(MountId),
    /// Remove every mount's link, then send the real folder to the Recycle Bin.
    DeleteBody(SkillId),
    /// Shelve one mount: remove its link, or move its real folder into the shelf.
    Disable(MountId),
    Restore(ShelfId),
    Bundle(BundleId, BundleOp),
    Jstack(JstackAction),
}

pub(crate) struct Plan {
    pub revision: u64,
    pub action: ManageAction,
    pub steps: Vec<Step>,
    pub effects: Vec<Effect>,
    pub blockers: Vec<Blocker>,
}

/// The only kinds of change skilltab makes. Each step carries what it expects to find on disk,
/// and `apply` refuses the whole plan when an unfinished step's expectation no longer holds.
#[derive(Clone, Debug)]
pub(crate) enum Step {
    /// Delete the reparse point, never what it points to. `shelve` is written first when the link can be put back.
    RemoveLink { link: LinkPath, target: PathBuf, shelve: Option<ShelfEntry> },
    /// Only a `RealDir` can be recycled, so a junction never reaches the Recycle Bin.
    Recycle { dir: RealDir, hash: ContentHash },
    Stash { dir: RealDir, hash: ContentHash, shelve: ShelfEntry, stash: PathBuf },
    Relink { shelf: ShelfId, entry: PathBuf, target: PathBuf },
    Unstash { shelf: ShelfId, stash: PathBuf, to: PathBuf },
    Run { command: ExternalCommand },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    HarnessLosesSkill { mount: MountId },
    HarnessGainsSkill { mount: MountId },
    /// Removing `via` also breaks a mount the user did not pick.
    AlsoBreaks { mount: MountId, via: PathBuf },
    BodyRecycled { dir: PathBuf },
    /// The command moves the jstack clone to a new commit, and every skill linked from the clone changes with it.
    UpdatesClone,
    /// Links jstack's skills into the harnesses and rewrites ~/.jstack-links.txt.
    RelinksJstack,
    InstallsDeclared { source: String, skills: Vec<String> },
}

/// Any blocker means apply refuses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Blocker {
    /// The owner puts the entry back, so a removal would silently revert. `name` is the entry's folder name.
    RecreatedBy { owner: Ownership, name: String },
    PartOfBundle { bundle: BundleId },
    /// The entry is the real folder, and these other mounts still reach it.
    SharedBody { dependents: Vec<MountId> },
    /// The folder is the user's own source in the jstack clone.
    InsideJstackRepo,
    HarnessBuiltin { harness: HarnessId },
    Unsupported { why: String, remedy: String },
}

// ---------------------------------------------------------------- planning (pure)

/// `repo` is the jstack clone as of the same refresh as `snap`, or None when there is no clone.
pub(crate) fn plan(snap: &Snapshot, shelf: &Shelf, repo: Option<&RepoState>, action: ManageAction) -> Plan {
    let mut p = Plan { revision: snap.revision, action: action.clone(), steps: Vec::new(), effects: Vec::new(), blockers: Vec::new() };
    match &action {
        ManageAction::RemoveMount(m) => take_mount(snap, shelf, m, ShelfReason::Removed, &mut p),
        ManageAction::Disable(m) => take_mount(snap, shelf, m, ShelfReason::Disabled, &mut p),
        ManageAction::DeleteBody(b) => delete_body(snap, b, &mut p),
        ManageAction::Restore(id) => restore(snap, id, &mut p),
        ManageAction::Bundle(b, op) => bundle(snap, b, *op, &mut p),
        ManageAction::Jstack(a) => jstack(repo, a, &mut p),
    }
    p
}

impl Plan {
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
            if matches!(body.origin, Origin::JstackRepo { .. }) {
                p.block(Blocker::InsideJstackRepo);
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
        Origin::JstackRepo { .. } => p.block(Blocker::InsideJstackRepo),
        Origin::Plugin { bundle } => p.block(Blocker::PartOfBundle { bundle: bundle.clone() }),
        Origin::HarnessBuiltin { harness } => p.block(Blocker::HarnessBuiltin { harness: *harness }),
        Origin::ClaudeAiSync { .. } => p.block(Blocker::RecreatedBy { owner: Ownership::ClaudeAiSync, name: folder_name(body.dir.as_path()) }),
        Origin::SkillsCli { .. } | Origin::Unknown => {}
    }
    let mut removed: Vec<String> = Vec::new();
    for m in &mounts {
        owner_blockers(m, p);
        p.effects.push(Effect::HarnessLosesSkill { mount: m.id.clone() });
        if let Some(link) = m.hops.first() {
            if !removed.contains(&norm_key(link.as_path())) {
                removed.push(norm_key(link.as_path()));
                p.steps.push(Step::RemoveLink { link: link.clone(), target: next_hop(m, body), shelve: None });
            }
        }
    }
    p.steps.push(Step::Recycle { dir: body.dir.clone(), hash: body.content_hash.clone() });
    p.effects.push(Effect::BodyRecycled { dir: body.dir.as_path().to_path_buf() });
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
            p.steps.push(Step::Relink { shelf: e.id.clone(), entry: e.entry.clone(), target: target.clone() });
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

fn jstack(repo: Option<&RepoState>, action: &JstackAction, p: &mut Plan) {
    let unsupported = |why: &str, remedy: &str| Blocker::Unsupported { why: why.to_owned(), remedy: remedy.to_owned() };
    let Some(repo) = repo else {
        return p.block(unsupported("No jstack clone was found on this machine.", r"Clone your jstack repo to ~\jstack, then rescan."));
    };
    match action {
        JstackAction::Sync if !repo.has_sync_script => {
            return p.block(unsupported("sync-skills.ps1 is not in your jstack clone.", "Pull a jstack commit that has it, then rescan."));
        }
        JstackAction::InstallDeclared { skills, .. } if skills.is_empty() => {
            return p.block(unsupported("No skills were named to install.", "Pick at least one skill from skill-sources.txt."));
        }
        _ => {}
    }
    let JstackCommand { command, updates_clone } = repo.command(action);
    if updates_clone {
        p.effects.push(Effect::UpdatesClone);
    }
    match action {
        JstackAction::Sync => p.effects.push(Effect::RelinksJstack),
        JstackAction::InstallDeclared { source, skills } => {
            p.effects.push(Effect::InstallsDeclared { source: source.clone(), skills: skills.clone() })
        }
        JstackAction::Pull => {}
    }
    p.steps.push(Step::Run { command });
}

/// Who would put the entry back, or who owns it instead of the user.
fn owner_blockers(m: &Mount, p: &mut Plan) {
    match &m.owner {
        Ownership::JstackSync | Ownership::ClaudeAiSync => {
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
        win::recycle(dir)
    }

    fn run(&self, c: &ExternalCommand) -> io::Result<String> {
        let command = |program: &str| {
            let mut cmd = std::process::Command::new(program);
            cmd.args(&c.args);
            if let Some(dir) = &c.cwd {
                cmd.current_dir(dir);
            }
            cmd
        };
        // npm installs CLIs as `<name>.cmd`, which Command does not find under the bare name.
        let out = match command(&c.program).output() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => command(&format!("{}.cmd", c.program)).output()?,
            other => other?,
        };
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
        Step::Relink { entry, target, .. } => still_link_to(entry, target),
        Step::Unstash { stash, to, .. } => !exists(stash) && exists(to),
        Step::Run { .. } => false,
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
        Step::Relink { entry, target, .. } => {
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
    }
}

fn stamped(e: &ShelfEntry) -> ShelfEntry {
    ShelfEntry { at: chrono::Utc::now(), ..e.clone() }
}

fn run_step(step: &Step, shelf: &mut Shelf, os: &dyn Os, output: &mut Vec<String>) -> io::Result<StepResult> {
    match step {
        Step::Relink { shelf: id, .. } | Step::Unstash { shelf: id, .. } if is_done(step) => {
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
            win::remove_link(link)?;
        }
        Step::Recycle { dir, .. } => os.recycle(dir)?,
        Step::Stash { dir, shelve, stash, .. } => {
            shelf.record(stamped(shelve))?;
            if let Some(parent) = stash.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(dir.as_path(), stash).map_err(|e| moved_error(e, stash))?;
        }
        Step::Relink { shelf: id, entry, target } => {
            win::create_junction(entry, target)?;
            shelf.forget(id)?;
        }
        Step::Unstash { shelf: id, stash, to } => {
            std::fs::rename(stash, to).map_err(|e| moved_error(e, to))?;
            shelf.forget(id)?;
        }
        Step::Run { command } => output.push(os.run(command)?),
    }
    Ok(StepResult::Done)
}

/// A rename cannot cross volumes. Copying and deleting instead would turn a cheap, atomic move into
/// a partial one, so the step fails and says why.
fn moved_error(e: io::Error, to: &Path) -> io::Error {
    const ERROR_NOT_SAME_DEVICE: i32 = 17;
    if e.raw_os_error() == Some(ERROR_NOT_SAME_DEVICE) {
        io::Error::other(format!("{} is on another drive, and skilltab only moves folders within one drive.", to.display()))
    } else {
        e
    }
}

#[cfg(test)]
pub(crate) mod tests;
