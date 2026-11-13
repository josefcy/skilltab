//! Builds a Snapshot. This is the only module that turns raw paths into LinkPath, RealDir,
//! SkillId and MountId. Everything downstream trusts those types (per boundary-discipline).

use crate::harness::{Discovery, EntryFact, Harness};
use crate::model::*;
use crate::provenance::Provenance;
use crate::Env;

/// Full rescan. About 300 entries and their SKILL.md files: it runs in milliseconds, so there is no inventory cache.
///
/// Steps:
/// 1. For each harness: `discover` gives EntryFacts and BundleFacts.
/// 2. For each entry: follow reparse points hop by hop (lstat + read_link, max 16 hops;
///    detect cycles), giving `hops: Vec<LinkPath>` and a terminal RealDir, or a ScanIssue::BrokenLink.
/// 3. Body identity = file id of the terminal dir. Entries that reach the same file id
///    share one SkillBody. This is where the 2-3 paths per skill collapse into one row.
/// 4. Per body: parse the frontmatter, compute content_hash, take the origin from provenance.
/// 5. Per mount: the owner comes from the harness hint, refined by provenance (jstack link
///    list, skills lock). Invocation keys come from the Naming and the manifest name, plus
///    SkillFile(canonical SKILL.md).
/// 6. Shadowing: within one harness, two Active mounts with the same Name key means the
///    harness keeps one: the lowest `EntryFact::rank`. The others become Shadowed.
/// 7. Add the shelf entries (owned by manage::Shelf) as disabled rows.
/// 8. Build SnapshotIndex.
pub fn scan(
    env: &Env,
    harnesses: &[&dyn Harness],
    projects: &[ProjectRoot],
    provenance: &Provenance,
    shelf: Vec<ShelfEntry>,
    revision: u64,
) -> Snapshot {
    unimplemented!()
}

/// The result of following one entry. Private: only `scan` builds the typed paths.
enum Resolved {
    Body { hops: Vec<LinkPath>, dir: RealDir, file_id: (u32, u64) },
    Broken { link: LinkPath, target: std::path::PathBuf },
    Cycle,
}

fn resolve_entry(entry: &std::path::Path) -> Resolved { unimplemented!() }

/// Re-check at apply time. The planner stores these and manage::apply calls them.
/// Returns false if the path is no longer the same kind of thing (or a link now points elsewhere).
pub(crate) fn still_link_to(link: &LinkPath, target: &std::path::Path) -> bool { unimplemented!() }
pub(crate) fn still_real_dir(dir: &RealDir, hash: &ContentHash) -> bool { unimplemented!() }

pub(crate) fn content_hash(dir: &RealDir) -> ContentHash { unimplemented!() }
