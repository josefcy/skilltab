//! Usage index. The only persisted copy of usage. It lives at %LOCALAPPDATA%\skilltab\usage.db
//! (SQLite, WAL), and only this module knows the schema.
//!
//! Stored: raw events keyed by the harness-native InvocationKey. They are never mapped to a
//! SkillId when stored. Matching to bodies happens when read, against the current snapshot.
//! So reinstalling, renaming, moving or bumping a plugin version needs no migration, and
//! usage of a skill that is gone stays visible as a "ghost".
//!
//! Schema (private):
//!   source(path PK, harness, file_id, size, mtime, offset, parser_state)
//!   event(harness, key_kind, key, kind, at, session, cwd, source_path, dedup, UNIQUE(harness, dedup))
//!   project(path PK, last_seen)          -- derived from event.cwd during ingest
//!   INDEX event(harness, key_kind, key, at)

use crate::harness::{Harness, InvocationKind};
use crate::model::{HarnessId, InvocationKey, ProjectRoot, Timestamp};
use crate::Env;
use std::collections::HashMap;
use std::path::Path;

pub struct UsageIndex {
    conn: rusqlite::Connection,
}

pub struct IngestProgress { pub files_done: u32, pub files_total: u32, pub bytes_done: u64, pub bytes_total: u64 }

pub struct IngestReport { pub files_read: u32, pub bytes_read: u64, pub events_added: u64, pub sources_reset: u32 }

#[derive(Default, Clone, Copy)]
pub struct Tally {
    pub total: u32,
    pub last_30d: u32,
    pub last_used: Option<Timestamp>,
    pub by_kind: [u32; 4], // indexed by InvocationKind as usize
}

pub struct UsagePoint { pub day: chrono::NaiveDate, pub harness: HarnessId, pub kind: InvocationKind, pub count: u32 }

impl UsageIndex {
    pub fn open(path: &Path) -> rusqlite::Result<UsageIndex> { unimplemented!() }

    /// Incremental and idempotent. It is safe to run twice, and safe to kill halfway.
    ///
    /// Per source (in parallel, one rayon task each; the inserts are funnelled to one writer):
    ///   stat → same file_id and size >= offset  : read [offset .. last '\n'], resume the parser from parser_state
    ///          different file_id or size < offset: delete the source's events, reparse from 0 (sources_reset += 1)
    ///          size == offset                   : skip (the common case: about 350 stats, nothing else)
    ///   One transaction per source: INSERT OR IGNORE the events, UPDATE source SET offset, parser_state.
    ///   The cursor moves only together with its rows, so a crash replays at most one file and dedup absorbs it.
    /// A source that is gone from `usage_sources` keeps its events (history outlives the deleted transcript).
    pub fn ingest(
        &mut self,
        harnesses: &[&dyn Harness],
        env: &Env,
        progress: &mut dyn FnMut(IngestProgress),
    ) -> rusqlite::Result<IngestReport> {
        unimplemented!()
    }

    /// One GROUP BY over the whole table. Tens of thousands of rows; this is fast enough on every refresh.
    pub fn totals(&self, now: Timestamp) -> rusqlite::Result<HashMap<(HarnessId, InvocationKey), Tally>> { unimplemented!() }

    /// Daily counts for the keys of one body (the snapshot supplies them through Mount::keys).
    pub fn series(&self, keys: &[(HarnessId, InvocationKey)]) -> rusqlite::Result<Vec<UsagePoint>> { unimplemented!() }

    /// Distinct cwd values seen in transcripts. Each one is normalized to its git root when it has one.
    pub fn known_projects(&self) -> rusqlite::Result<Vec<ProjectRoot>> { unimplemented!() }
}
