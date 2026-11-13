//! Follows one entry through junctions and symlinks, one hop at a time. The only place that
//! mints `LinkPath` and `RealDir`.

use crate::model::{norm_key, LinkPath, RealDir};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MAX_HOPS: usize = 16;

pub(super) type FileId = (u32, u64);

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

pub(super) fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\").or_else(|| s.strip_prefix(r"\??\")) {
        PathBuf::from(rest)
    } else {
        p
    }
}

/// (volume serial, file index) of a directory. Two paths with the same pair are one folder.
fn file_id(dir: &Path) -> io::Result<FileId> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let handle = fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(dir)?;
    // SAFETY: BY_HANDLE_FILE_INFORMATION is plain data that the call fills in, and the handle
    // stays open for the duration of the call.
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetFileInformationByHandle(handle.as_raw_handle() as HANDLE, &mut info) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((info.dwVolumeSerialNumber, (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
}

/// Re-check at apply time: is `link` still a link, and does it still point at `target`?
pub(crate) fn still_link_to(link: &LinkPath, target: &Path) -> bool {
    let is_link = fs::symlink_metadata(link.as_path()).map(|m| m.file_type().is_symlink()).unwrap_or(false);
    if !is_link {
        return false;
    }
    match fs::read_link(link.as_path()) {
        Ok(t) => norm_key(&absolute_target(link.as_path(), t)) == norm_key(target),
        Err(_) => false,
    }
}
