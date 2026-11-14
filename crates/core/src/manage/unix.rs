//! The only code that links or trashes on Linux and macOS. Private to `manage`, so nothing can
//! change the disk without going through a plan.

use crate::model::{LinkPath, RealDir};
use std::io;
use std::path::{Path, PathBuf};

pub(crate) const TRASH_NAME: &str = "Trash";

/// Deletes the symlink itself. `remove_file` never follows it, so the target is untouched.
pub(super) fn remove_link(link: &LinkPath) -> io::Result<()> {
    std::fs::remove_file(link.as_path())
}

/// Sends the folder to the user's trash.
pub(super) fn recycle(dir: &RealDir) -> io::Result<()> {
    trash::delete(dir.as_path()).map_err(io::Error::other)
}

/// Creates `entry` as a symlink to the absolute `target`.
pub(super) fn create_link(entry: &Path, target: &Path) -> io::Result<()> {
    if !target.is_absolute() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a link target must be absolute"));
    }
    std::os::unix::fs::symlink(target, entry)
}

pub(super) fn tool_names(program: &str) -> Vec<String> {
    vec![program.to_owned()]
}

pub(super) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Where tools live that a Finder or desktop launched app misses, because it starts with a short PATH.
pub(super) fn extra_tool_dirs(home: Option<&Path>) -> Vec<PathBuf> {
    let under_home = |rel: &str| home.map(|h| h.join(rel));
    [under_home(".local/bin"), Some("/opt/homebrew/bin".into()), Some("/usr/local/bin".into()), under_home(".npm-global/bin")]
        .into_iter()
        .flatten()
        .collect()
}
