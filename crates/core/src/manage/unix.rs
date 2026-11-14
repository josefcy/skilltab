//! The only code that links or trashes on Linux and macOS. Private to `manage`, so nothing can
//! change the disk without going through a plan.

use crate::model::{LinkPath, RealDir};
use std::io;
use std::path::Path;

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
