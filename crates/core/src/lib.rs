//! skilltab-core. A plain Rust library with no Tauri dependency.
//!
//! Unit 1 builds a `Snapshot` of every installed skill. The modules stay private so the shell
//! can only go through what is re-exported here.

// Fields, stubs and helpers here are consumed by the usage, manage and view units.
#![allow(dead_code, unused_variables)]

mod fsutil;
mod harness;
mod model;
mod provenance;

use std::path::PathBuf;

pub struct Env {
    pub home: PathBuf,
    /// Project roots to check for `.claude/skills`, `.agents/skills` and `.codex/skills`.
    pub projects: Vec<PathBuf>,
    /// The jstack clone. None means no skill is attributed to jstack.
    pub jstack_root: Option<PathBuf>,
}
