//! Tree hash over a skill folder.

use crate::model::{ContentHash, RealDir};
use sha1::{Digest, Sha1};
use std::fs;
use std::path::{Path, PathBuf};

/// Dependency and VCS folders change without the skill changing, and can be huge.
const SKIPPED_DIRS: &[&str] = &[".git", "node_modules"];

pub(crate) fn content_hash(dir: &RealDir) -> ContentHash {
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    collect(dir.as_path(), "", &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut h = Sha1::new();
    for (rel, abs) in &files {
        h.update(rel.as_bytes());
        h.update([0]);
        match fs::read(abs) {
            Ok(bytes) => {
                h.update((bytes.len() as u64).to_le_bytes());
                h.update(&bytes);
            }
            Err(_) => h.update(u64::MAX.to_le_bytes()),
        }
    }
    ContentHash(h.finalize().into())
}

/// A link inside the folder is hashed by name only, so a cycle or a link out of the folder
/// cannot pull foreign content into the hash.
fn collect(dir: &Path, rel: &str, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read) = fs::read_dir(dir) else { return };
    for item in read.flatten() {
        let name = item.file_name().to_string_lossy().into_owned();
        let rel_path = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        let Ok(ft) = item.file_type() else { continue };
        if ft.is_symlink() {
            out.push((format!("{rel_path}\0link"), PathBuf::new()));
        } else if ft.is_dir() {
            if !SKIPPED_DIRS.contains(&name.as_str()) {
                collect(&item.path(), &rel_path, out);
            }
        } else {
            out.push((rel_path, item.path()));
        }
    }
}
