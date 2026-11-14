//! Follows one entry through junctions and symlinks, one hop at a time. The only place that
//! mints `LinkPath` and `RealDir`.

use crate::fsutil::{file_id, strip_verbatim, FileId};
use crate::model::{norm_key, LinkPath, RealDir};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_HOPS: usize = 16;

/// The result of following one entry.
pub(super) enum Resolved {
    Body { hops: Vec<LinkPath>, dir: RealDir, file_id: FileId },
    Broken { link: LinkPath, target: PathBuf },
    Cycle,
    Unreadable { path: PathBuf, error: String },
}

pub(super) fn resolve_entry(entry: &Path) -> Resolved {
    let mut hops: Vec<LinkPath> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut cur = entry.to_path_buf();

    for _ in 0..=MAX_HOPS {
        let meta = match fs::symlink_metadata(&cur) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return match hops.last() {
                    Some(link) => Resolved::Broken { link: link.clone(), target: cur },
                    None => Resolved::Unreadable { path: cur, error: e.to_string() },
                };
            }
            Err(e) => return Resolved::Unreadable { path: cur, error: e.to_string() },
        };

        if meta.file_type().is_symlink() {
            if !seen.insert(norm_key(&cur)) {
                return Resolved::Cycle;
            }
            let target = match fs::read_link(&cur) {
                Ok(t) => t,
                Err(e) => return Resolved::Unreadable { path: cur, error: e.to_string() },
            };
            let next = canonical_parent(&absolute_target(&cur, target));
            hops.push(LinkPath::mint(cur));
            cur = next;
            continue;
        }

        if !meta.is_dir() {
            return Resolved::Unreadable { path: cur, error: "not a directory".to_owned() };
        }
        if !cur.join("SKILL.md").is_file() {
            return Resolved::Unreadable { path: cur, error: "no SKILL.md".to_owned() };
        }
        let canonical = match fs::canonicalize(&cur) {
            Ok(c) => strip_verbatim(c),
            Err(e) => return Resolved::Unreadable { path: cur, error: e.to_string() },
        };
        return match file_id(&canonical) {
            Ok(file_id) => Resolved::Body { hops, dir: RealDir::mint(canonical), file_id },
            Err(e) => Resolved::Unreadable { path: canonical, error: e.to_string() },
        };
    }
    Resolved::Cycle
}

fn absolute_target(link: &Path, target: PathBuf) -> PathBuf {
    let target = strip_verbatim(target);
    if target.is_relative() {
        link.parent().map(|p| p.join(&target)).unwrap_or(target)
    } else {
        target
    }
}

/// A link's stored target can spell its parents with a different case or through another link.
/// Canonicalizing the parent makes the next hop's spelling match what a scan of that folder sees.
fn canonical_parent(target: &Path) -> PathBuf {
    match (target.parent(), target.file_name()) {
        (Some(parent), Some(name)) => match fs::canonicalize(parent) {
            Ok(p) => strip_verbatim(p).join(name),
            Err(_) => target.to_path_buf(),
        },
        _ => target.to_path_buf(),
    }
}

/// Where the link at `path` points, spelled as a scan spells the next hop. None when `path` is not a link.
fn link_target(path: &Path) -> Option<PathBuf> {
    let is_link = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return None;
    }
    let target = fs::read_link(path).ok()?;
    Some(canonical_parent(&absolute_target(path, target)))
}

/// Re-check at apply time: is `link` still a link, and does it still point at `target`?
pub(crate) fn still_link_to(link: &Path, target: &Path) -> bool {
    link_target(link).is_some_and(|t| norm_key(&t) == norm_key(target))
}
