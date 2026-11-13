//! Answers "where did this folder come from" and "who made this link", from the record
//! files other tools keep. skilltab reads these files and never writes them.
//!   ~/.agents/.skill-lock.json (v3)    skills CLI installs
//!   ~/.jstack-links.txt                junctions that jstack's sync owns and recreates
//!   jstack clone path                  bodies inside it are Origin::JstackRepo
//!   ~/.claude/skills/synced/*/manifest.json
//! Plugin and harness-builtin facts come from the harness's own discovery.

use crate::model::*;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub struct Provenance {
    lock: HashMap<String, LockEntry>,
    jstack_links: HashSet<PathBuf>,
    jstack_skills_dir: Option<PathBuf>,
    synced: HashMap<PathBuf, String>,
}

struct LockEntry { source: String, source_url: Option<String>, git_ref: Option<String>, skill_folder_hash: String }

impl Provenance {
    /// A missing or unreadable file means "no facts from it". It never fails the scan.
    /// A lock version other than 3 is read as no facts, and a ScanIssue is raised.
    pub fn load(env: &crate::Env, jstack_root: Option<&Path>) -> Provenance { unimplemented!() }

    /// Order matters. The first match wins:
    /// inside the jstack skills dir > synced manifest > plugin (from the harness fact) >
    /// skills lock (by folder name under ~/.agents/skills) > Unknown.
    pub fn origin(&self, dir: &RealDir, harness_fact: Option<&Origin>) -> Origin { unimplemented!() }

    /// Only refines an Unmanaged hint. The harness's own facts (PluginManager, builtin) win.
    pub fn owner(&self, entry: &Path, hint: Ownership) -> Ownership { unimplemented!() }
}
