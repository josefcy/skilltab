//! Project roots, found from the `cwd` in the first lines of Claude transcripts, never from folder names.
//! The usage index will own this once it tracks transcripts incrementally. Until then each refresh rereads the heads.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

const TRANSCRIPT_LINES: usize = 8;
const TRANSCRIPT_HEAD_BYTES: u64 = 256 * 1024;

/// Real project roots named by transcript `cwd` values: each cwd walked up to its git root.
pub fn discover_projects(home: &Path) -> Vec<PathBuf> {
    let mut cwds: BTreeSet<PathBuf> = BTreeSet::new();
    let Ok(slugs) = std::fs::read_dir(home.join(".claude").join("projects")) else { return Vec::new() };
    for slug in slugs.flatten().filter(|s| s.path().is_dir()) {
        let Ok(files) = std::fs::read_dir(slug.path()) else { continue };
        for f in files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|e| e == "jsonl")) {
            cwds.extend(first_cwds(&f));
        }
    }

    let mut roots: BTreeMap<String, PathBuf> = BTreeMap::new();
    for cwd in cwds.into_iter().filter(|c| c.is_dir()) {
        let root = cwd.ancestors().find(|a| a.join(".git").exists()).unwrap_or(&cwd).to_path_buf();
        if root != home {
            roots.entry(root.to_string_lossy().to_lowercase()).or_insert(root);
        }
    }
    roots.into_values().collect()
}

/// Reads only a bounded head of the file and only the first few lines, and keeps only `cwd`.
fn first_cwds(transcript: &Path) -> Vec<PathBuf> {
    let Ok(file) = File::open(transcript) else { return Vec::new() };
    let mut reader = BufReader::new(file.take(TRANSCRIPT_HEAD_BYTES));
    let mut out = Vec::new();
    let mut buf = Vec::new();
    for _ in 0..TRANSCRIPT_LINES {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if let Ok(v) = serde_json::from_slice::<Value>(&buf)
            && let Some(cwd) = v.get("cwd").and_then(|c| c.as_str())
        {
            out.push(PathBuf::from(cwd));
        }
    }
    out
}
