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
//!   event(id, harness, key_kind, key, kind, at, session, cwd, dedup, UNIQUE(harness, dedup))
//!   INDEX event(harness, key_kind, key, at)
//! `at` is Unix milliseconds. `key_kind` is 0 for Name and 1 for SkillFile. `kind` is the
//! `InvocationKind` discriminant. A source's rows and its cursor move in one transaction.

use crate::fsutil;
use crate::harness::{Harness, InvocationKind, ParserState, RawEvent, SessionParser};
use crate::model::{norm_key, HarnessId, InvocationKey, Tally, Timestamp, TrackingSince};
use crate::Env;
use rayon::prelude::*;
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: i64 = 1;
const READ_CHUNK: usize = 1 << 20;
const RECENT_DAYS: i64 = 30;
const KEY_NAME: i64 = 0;
const KEY_SKILL_FILE: i64 = 1;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS source (
    path TEXT PRIMARY KEY,
    harness TEXT NOT NULL,
    file_id TEXT NOT NULL,
    size INTEGER NOT NULL,
    mtime INTEGER NOT NULL,
    offset INTEGER NOT NULL,
    parser_state TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS event (
    id INTEGER PRIMARY KEY,
    harness TEXT NOT NULL,
    key_kind INTEGER NOT NULL,
    key TEXT NOT NULL,
    kind INTEGER NOT NULL,
    at INTEGER NOT NULL,
    session TEXT NOT NULL,
    cwd TEXT,
    dedup TEXT NOT NULL,
    UNIQUE (harness, dedup)
);
CREATE INDEX IF NOT EXISTS event_key ON event (harness, key_kind, key, at);
";

#[derive(Debug)]
pub enum UsageError {
    Db(rusqlite::Error),
    Io(io::Error),
    /// The database was written by a newer skilltab.
    Schema(i64),
}

impl fmt::Display for UsageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UsageError::Db(e) => write!(f, "usage database: {e}"),
            UsageError::Io(e) => write!(f, "usage database folder: {e}"),
            UsageError::Schema(v) => write!(f, "usage database has schema {v}, this build reads {SCHEMA_VERSION}"),
        }
    }
}

impl std::error::Error for UsageError {}

impl From<rusqlite::Error> for UsageError {
    fn from(e: rusqlite::Error) -> Self {
        UsageError::Db(e)
    }
}

impl From<io::Error> for UsageError {
    fn from(e: io::Error) -> Self {
        UsageError::Io(e)
    }
}

pub type Result<T> = std::result::Result<T, UsageError>;

pub struct UsageIndex {
    conn: Connection,
}

#[derive(Debug, Clone, Copy)]
pub struct IngestProgress {
    pub files_done: u32,
    pub files_total: u32,
    pub bytes_done: u64,
    pub bytes_total: u64,
}

#[derive(Debug, Default)]
pub struct IngestReport {
    /// Sources that had something new to read (not counting the ones skipped on size and mtime).
    pub files_read: u32,
    pub bytes_read: u64,
    pub events_added: u64,
    pub sources_reset: u32,
    /// Sources seen this run, whether or not they were read.
    pub sources_seen: u32,
    /// Files that could not be read this run. The rest of the run is not affected, and the
    /// source is retried next time.
    pub errors: Vec<(PathBuf, String)>,
}

pub struct UsagePoint {
    pub day: chrono::NaiveDate,
    pub harness: HarnessId,
    pub kind: InvocationKind,
    pub count: u32,
}

struct SourceRow {
    file_id: String,
    size: u64,
    mtime: i64,
    offset: u64,
    parser_state: String,
}

/// One source that needs reading, with where to start.
struct Job<'a> {
    harness: &'a dyn Harness,
    path: PathBuf,
    file_id: String,
    size: u64,
    mtime: i64,
    start: u64,
    resume: Option<ParserState>,
    reset: bool,
}

struct Parsed {
    events: Vec<RawEvent>,
    /// Just past the last newline consumed.
    offset: u64,
    /// End of the file as read. Can be past `offset` when the last line is still being written.
    size: u64,
    state: ParserState,
}

impl UsageIndex {
    pub fn open(path: &Path) -> Result<UsageIndex> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(UsageError::Schema(version));
        }
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        Ok(UsageIndex { conn })
    }

    /// Ingests every registered harness.
    pub fn ingest_all(&mut self, env: &Env, progress: &mut dyn FnMut(IngestProgress)) -> Result<IngestReport> {
        self.ingest(crate::harness::registry(), env, progress)
    }

    /// Incremental and idempotent. It is safe to run twice, and safe to kill halfway.
    ///
    /// Per source: same file id and size >= offset resumes at the stored offset. A different file
    /// id or a smaller file starts again from 0. A source that is gone from `usage_sources` keeps
    /// its events, because history outlives the deleted transcript. A reset keeps old events too:
    /// the re-read repeats the same dedup keys, so counts stay right.
    /// Files are parsed in parallel, and one thread writes. A source's rows and cursor commit
    /// together, so a crash replays at most one file and dedup absorbs it.
    pub fn ingest(
        &mut self,
        harnesses: &[&dyn Harness],
        env: &Env,
        progress: &mut dyn FnMut(IngestProgress),
    ) -> Result<IngestReport> {
        let known = self.load_sources()?;
        let mut report = IngestReport::default();
        let mut jobs: Vec<Job> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for h in harnesses {
            for source in h.usage_sources(env) {
                let key = source.path.to_string_lossy().into_owned();
                if !seen.insert(key.clone()) {
                    continue;
                }
                report.sources_seen += 1;
                match plan(*h, source.path, source.append_only, known.get(&key)) {
                    Ok(Some(job)) => jobs.push(job),
                    Ok(None) => {}
                    Err((path, e)) => report.errors.push((path, e.to_string())),
                }
            }
        }

        let mut p = IngestProgress {
            files_done: 0,
            files_total: jobs.len() as u32,
            bytes_done: 0,
            bytes_total: jobs.iter().map(|j| j.size.saturating_sub(j.start)).sum(),
        };
        progress(p);

        let (tx, rx) = std::sync::mpsc::channel::<(usize, io::Result<Parsed>)>();
        let jobs_ref = &jobs;
        let conn = &mut self.conn;
        std::thread::scope(|scope| -> Result<()> {
            scope.spawn(move || {
                jobs_ref.par_iter().enumerate().for_each_with(tx, |tx, (i, job)| {
                    let _ = tx.send((i, read_source(job)));
                });
            });
            for (i, parsed) in rx {
                let job = &jobs[i];
                p.files_done += 1;
                p.bytes_done += job.size.saturating_sub(job.start);
                match parsed {
                    Ok(parsed) => {
                        report.files_read += 1;
                        report.bytes_read += parsed.size.saturating_sub(job.start);
                        report.events_added += store(conn, job, &parsed)?;
                        if job.reset {
                            report.sources_reset += 1;
                        }
                    }
                    Err(e) => report.errors.push((job.path.clone(), e.to_string())),
                }
                progress(p);
            }
            Ok(())
        })?;
        Ok(report)
    }

    fn load_sources(&self) -> Result<HashMap<String, SourceRow>> {
        let mut stmt = self.conn.prepare("SELECT path, file_id, size, mtime, offset, parser_state FROM source")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                SourceRow {
                    file_id: r.get(1)?,
                    size: r.get::<_, i64>(2)? as u64,
                    mtime: r.get(3)?,
                    offset: r.get::<_, i64>(4)? as u64,
                    parser_state: r.get(5)?,
                },
            ))
        })?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn event_count(&self) -> Result<u64> {
        Ok(self.conn.query_row("SELECT COUNT(*) FROM event", [], |r| r.get::<_, i64>(0))? as u64)
    }

    /// One GROUP BY over the whole table. Tens of thousands of rows; this is fast enough on every refresh.
    pub fn totals(&self, now: Timestamp) -> Result<HashMap<(HarnessId, InvocationKey), Tally>> {
        let recent_from = (now - chrono::Duration::days(RECENT_DAYS)).timestamp_millis();
        let mut stmt = self.conn.prepare(
            "SELECT harness, key_kind, key, kind, COUNT(*), SUM(at >= ?1), MAX(at) FROM event
             GROUP BY harness, key_kind, key, kind",
        )?;
        let rows = stmt.query_map([recent_from], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut out: HashMap<(HarnessId, InvocationKey), Tally> = HashMap::new();
        for row in rows {
            let (harness, key_kind, key, kind, count, recent, last) = row?;
            let (Some(harness), Some(kind), Some(key)) =
                (HarnessId::parse(&harness), InvocationKind::from_index(kind), decode_key(key_kind, key))
            else {
                continue;
            };
            let tally = out.entry((harness, key)).or_default();
            let count = count as u32;
            tally.total += count;
            tally.last_30d += recent as u32;
            tally.by_kind[kind as usize] += count;
            let last = chrono::DateTime::from_timestamp_millis(last);
            tally.last_used = tally.last_used.max(last);
        }
        Ok(out)
    }

    /// Daily counts for the keys of one body (the snapshot supplies them through Mount::keys).
    /// Days are UTC. Counts of keys that fall on the same day, harness and kind are added.
    pub fn series(&self, keys: &[(HarnessId, InvocationKey)]) -> Result<Vec<UsagePoint>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT date(at / 1000, 'unixepoch'), kind, COUNT(*) FROM event
             WHERE harness = ?1 AND key_kind = ?2 AND key = ?3 GROUP BY 1, 2",
        )?;
        let mut merged: BTreeMap<(chrono::NaiveDate, HarnessId, usize), u32> = BTreeMap::new();
        for (harness, key) in keys {
            let (key_kind, text) = encode_key(key);
            let rows = stmt.query_map(params![harness.as_str(), key_kind, text], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?))
            })?;
            for row in rows {
                let (day, kind, count) = row?;
                let (Ok(day), Some(kind)) = (day.parse::<chrono::NaiveDate>(), InvocationKind::from_index(kind)) else {
                    continue;
                };
                *merged.entry((day, *harness, kind as usize)).or_default() += count as u32;
            }
        }
        Ok(merged
            .into_iter()
            .map(|((day, harness, kind), count)| UsagePoint { day, harness, kind: InvocationKind::ALL[kind], count })
            .collect())
    }

    /// Working folders of the sessions that used a skill, each walked up to its git root when it has one.
    /// Only folders that still exist are returned. They are candidates for `Env::projects`: only the
    /// scanner mints `ProjectRoot`s, and the caller decides whether the home folder counts as a project.
    pub fn known_projects(&self) -> Result<Vec<PathBuf>> {
        let mut stmt = self.conn.prepare("SELECT DISTINCT cwd FROM event WHERE cwd IS NOT NULL")?;
        let cwds: Vec<String> = stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
        let mut roots: BTreeMap<String, PathBuf> = BTreeMap::new();
        for cwd in cwds.into_iter().map(PathBuf::from).filter(|c| c.is_dir()) {
            let root = cwd.ancestors().find(|a| a.join(".git").exists()).unwrap_or(&cwd).to_path_buf();
            roots.entry(norm_key(&root)).or_insert(root);
        }
        Ok(roots.into_values().collect())
    }

    /// Earliest event time per harness. Before that date nothing is known, so a skill with no
    /// events is "unused" only for the period after it.
    pub fn tracking_since(&self) -> Result<TrackingSince> {
        let mut stmt = self.conn.prepare("SELECT harness, MIN(at) FROM event GROUP BY harness")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out = TrackingSince::new();
        for row in rows {
            let (harness, at) = row?;
            if let (Some(harness), Some(at)) = (HarnessId::parse(&harness), chrono::DateTime::from_timestamp_millis(at)) {
                out.insert(harness, at);
            }
        }
        Ok(out)
    }
}

fn encode_key(key: &InvocationKey) -> (i64, String) {
    match key {
        InvocationKey::Name(n) => (KEY_NAME, n.clone()),
        InvocationKey::SkillFile(p) => (KEY_SKILL_FILE, p.to_string_lossy().into_owned()),
    }
}

fn decode_key(kind: i64, key: String) -> Option<InvocationKey> {
    match kind {
        KEY_NAME => Some(InvocationKey::Name(key)),
        KEY_SKILL_FILE => Some(InvocationKey::SkillFile(PathBuf::from(key))),
        _ => None,
    }
}

/// Decides what to do with one source: `None` means nothing new. Both the size and the modified
/// time are compared, so a file that ends in a half-written line is not re-read on every run.
fn plan<'a>(
    harness: &'a dyn Harness,
    path: PathBuf,
    append_only: bool,
    row: Option<&SourceRow>,
) -> std::result::Result<Option<Job<'a>>, (PathBuf, io::Error)> {
    let stat = |e: io::Error| (path.clone(), e);
    let meta = match std::fs::metadata(&path) {
        Ok(m) => m,
        // Deleted between the directory listing and now. Its stored events stay.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(stat(e)),
    };
    let (vol, index) = fsutil::file_id(&path).map_err(stat)?;
    let file_id = format!("{vol:x}:{index:x}");
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_micros() as i64);

    let (start, resume, reset) = match row {
        None => (0, None, false),
        Some(r) if r.file_id != file_id || size < r.offset => (0, None, true),
        Some(r) if size == r.size && mtime == r.mtime => return Ok(None),
        Some(r) if append_only && size == r.offset => return Ok(None),
        Some(r) if append_only => (r.offset, Some(ParserState(r.parser_state.clone())), false),
        Some(_) => (0, None, false),
    };
    Ok(Some(Job { harness, path, file_id, size, mtime, start, resume, reset }))
}

/// Reads from `job.start` to the end of the file and feeds every complete line to a fresh parser.
/// A trailing line with no newline yet is left for the next run.
fn read_source(job: &Job) -> io::Result<Parsed> {
    let mut file = File::open(&job.path)?;
    file.seek(SeekFrom::Start(job.start))?;
    let mut parser: Box<dyn SessionParser> = job.harness.session_parser(job.resume.as_ref());
    let mut events = Vec::new();
    let mut chunk = vec![0u8; READ_CHUNK];
    let mut carry: Vec<u8> = Vec::new();
    let mut offset = job.start;
    let mut end = job.start;
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        end += n as u64;
        carry.extend_from_slice(&chunk[..n]);
        let mut line_start = 0;
        while let Some(nl) = memchr::memchr(b'\n', &carry[line_start..]) {
            let line = &carry[line_start..line_start + nl];
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            if !line.is_empty() {
                parser.feed(line, &mut events);
            }
            line_start += nl + 1;
        }
        offset += line_start as u64;
        carry.drain(..line_start);
    }
    Ok(Parsed { events, offset, size: end, state: parser.checkpoint() })
}

/// Inserts the events and moves the cursor in one transaction. Returns how many events were new.
fn store(conn: &mut Connection, job: &Job, parsed: &Parsed) -> Result<u64> {
    let tx = conn.transaction()?;
    let mut added = 0u64;
    {
        let mut insert = tx.prepare_cached(
            "INSERT OR IGNORE INTO event (harness, key_kind, key, kind, at, session, cwd, dedup)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )?;
        for e in &parsed.events {
            let (key_kind, key) = encode_key(&e.key);
            added += insert.execute(params![
                job.harness.id().as_str(),
                key_kind,
                key,
                e.kind as i64,
                e.at.timestamp_millis(),
                e.session,
                e.cwd.as_ref().map(|c| c.to_string_lossy().into_owned()),
                e.dedup,
            ])? as u64;
        }
    }
    tx.execute(
        "INSERT INTO source (path, harness, file_id, size, mtime, offset, parser_state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT (path) DO UPDATE SET harness = ?2, file_id = ?3, size = ?4, mtime = ?5, offset = ?6, parser_state = ?7",
        params![
            job.path.to_string_lossy(),
            job.harness.id().as_str(),
            job.file_id,
            parsed.size as i64,
            job.mtime,
            parsed.offset as i64,
            parsed.state.0,
        ],
    )?;
    tx.commit()?;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::registry;
    use serde_json::json;
    use std::fs;
    use std::io::Write;

    struct Fixture {
        home: tempfile::TempDir,
        db: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Fixture {
            Fixture { home: tempfile::tempdir().unwrap(), db: tempfile::tempdir().unwrap() }
        }
        fn env(&self) -> Env {
            Env { home: self.home.path().to_path_buf(), projects: vec![] }
        }
        fn index(&self) -> UsageIndex {
            UsageIndex::open(&self.db.path().join("usage.db")).unwrap()
        }
        fn transcript(&self, slug: &str, name: &str) -> PathBuf {
            let dir = self.home.path().join(".claude").join("projects").join(slug);
            fs::create_dir_all(&dir).unwrap();
            dir.join(name)
        }
        fn ingest(&self, index: &mut UsageIndex) -> IngestReport {
            index.ingest(registry(), &self.env(), &mut |_| {}).unwrap()
        }
    }

    fn tool_use(id: &str, skill: &str, ts: &str) -> String {
        json!({"type": "assistant", "uuid": format!("a-{id}"), "sessionId": "s", "cwd": "D:\\p", "timestamp": ts,
            "message": {"content": [{"type": "tool_use", "id": id, "name": "Skill", "input": {"skill": skill}}]}})
        .to_string()
    }

    fn slash(uuid: &str, name: &str, ts: &str) -> String {
        json!({"type": "user", "uuid": uuid, "sessionId": "s", "cwd": "D:\\p", "timestamp": ts,
            "message": {"content": format!("<command-message>{name}</command-message>\n<command-name>/{name}</command-name>")}})
        .to_string()
    }

    fn write_lines(path: &Path, lines: &[String]) {
        let mut text = lines.join("\n");
        text.push('\n');
        fs::write(path, text).unwrap();
    }

    fn append(path: &Path, text: &str) {
        fs::OpenOptions::new().append(true).open(path).unwrap().write_all(text.as_bytes()).unwrap();
    }

    fn count(index: &UsageIndex, name: &str) -> u32 {
        let now = chrono::Utc::now();
        let key = (HarnessId::CLAUDE_CODE, InvocationKey::Name(name.into()));
        index.totals(now).unwrap().get(&key).map_or(0, |t| t.total)
    }

    const T0: &str = "2026-11-01T10:00:00.000Z";

    #[test]
    fn a_second_ingest_adds_nothing_and_reads_nothing() {
        let f = Fixture::new();
        write_lines(&f.transcript("p", "a.jsonl"), &[tool_use("t1", "x", T0), slash("u1", "y", T0)]);
        let mut index = f.index();

        let first = f.ingest(&mut index);
        assert_eq!((first.files_read, first.events_added), (1, 2));

        let second = f.ingest(&mut index);
        assert_eq!((second.files_read, second.bytes_read, second.events_added), (0, 0, 0));
        assert_eq!(index.event_count().unwrap(), 2);
    }

    #[test]
    fn appended_lines_are_counted_once_and_only_new_bytes_are_read() {
        let f = Fixture::new();
        let path = f.transcript("p", "a.jsonl");
        write_lines(&path, &[tool_use("t1", "x", T0)]);
        let mut index = f.index();
        let first = f.ingest(&mut index);

        let added = format!("{}\n{}\n", tool_use("t2", "x", T0), slash("u1", "x", T0));
        append(&path, &added);
        let second = f.ingest(&mut index);

        assert_eq!(second.events_added, 2);
        assert_eq!(second.bytes_read, added.len() as u64);
        assert!(first.bytes_read < second.bytes_read);
        assert_eq!(count(&index, "x"), 3);
    }

    #[test]
    fn a_line_cut_off_mid_write_waits_for_its_newline() {
        let f = Fixture::new();
        let path = f.transcript("p", "a.jsonl");
        write_lines(&path, &[tool_use("t1", "x", T0)]);
        let mut index = f.index();
        f.ingest(&mut index);

        let line = tool_use("t2", "x", T0);
        let (head, tail) = line.split_at(line.len() / 2);
        append(&path, head);
        assert_eq!(f.ingest(&mut index).events_added, 0);
        assert_eq!(f.ingest(&mut index).events_added, 0);
        assert_eq!(count(&index, "x"), 1);

        append(&path, &format!("{tail}\n"));
        assert_eq!(f.ingest(&mut index).events_added, 1);
        assert_eq!(count(&index, "x"), 2);
    }

    #[test]
    fn a_shrunk_file_is_read_again_from_the_start_without_double_counting() {
        let f = Fixture::new();
        let path = f.transcript("p", "a.jsonl");
        write_lines(&path, &[tool_use("t1", "x", T0), tool_use("t2", "x", T0), tool_use("t3", "x", T0)]);
        let mut index = f.index();
        f.ingest(&mut index);

        write_lines(&path, &[tool_use("t1", "x", T0), tool_use("t9", "z", T0)]);
        let report = f.ingest(&mut index);

        assert_eq!(report.sources_reset, 1);
        assert_eq!(report.events_added, 1);
        assert_eq!(count(&index, "x"), 3, "history from the longer file is kept");
        assert_eq!(count(&index, "z"), 1);
    }

    #[test]
    fn the_same_events_in_two_files_count_once() {
        let f = Fixture::new();
        let lines = [tool_use("t1", "x", T0), slash("u1", "x", T0)];
        write_lines(&f.transcript("p", "a.jsonl"), &lines);
        write_lines(&f.transcript("p", "resumed.jsonl"), &lines);
        let mut index = f.index();

        let report = f.ingest(&mut index);

        assert_eq!(report.files_read, 2);
        assert_eq!(report.events_added, 2);
        assert_eq!(count(&index, "x"), 2);
    }

    #[test]
    fn tool_definition_lines_are_not_counted() {
        let f = Fixture::new();
        let desc = "Run <command-name>/x</command-name> through the \"name\":\"Skill\" tool";
        let lines = [
            json!({"type": "attachment", "uuid": "c1", "timestamp": T0, "attachment": {"tools": [{"name": "Skill", "description": desc}]}}).to_string(),
            json!({"type": "api-request-shape", "timestamp": T0, "shape": {"tools": [{"name": "Skill", "description": desc}]}}).to_string(),
            tool_use("t1", "real", T0),
        ];
        write_lines(&f.transcript("p", "a.jsonl"), &lines);
        let mut index = f.index();

        f.ingest(&mut index);

        assert_eq!(index.event_count().unwrap(), 1);
        assert_eq!(count(&index, "real"), 1);
    }

    #[test]
    fn totals_split_by_kind_and_window_and_series_group_by_day() {
        let f = Fixture::new();
        let now = chrono::Utc::now();
        let old = (now - chrono::Duration::days(40)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let recent = (now - chrono::Duration::days(2)).to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        write_lines(
            &f.transcript("p", "a.jsonl"),
            &[
                tool_use("t1", "x", &old),
                tool_use("t2", "x", &recent),
                slash("u1", "x", &recent),
                tool_use("t3", "other", &recent),
            ],
        );
        let mut index = f.index();
        f.ingest(&mut index);

        let key = (HarnessId::CLAUDE_CODE, InvocationKey::Name("x".into()));
        let tally = index.totals(now).unwrap()[&key];
        assert_eq!(tally.total, 3);
        assert_eq!(tally.last_30d, 2);
        assert_eq!(tally.by_kind[InvocationKind::Model as usize], 2);
        assert_eq!(tally.by_kind[InvocationKind::Slash as usize], 1);
        assert_eq!(tally.last_used.unwrap().date_naive(), (now - chrono::Duration::days(2)).date_naive());

        let series = index.series(&[key]).unwrap();
        let days: Vec<_> = series.iter().map(|p| (p.kind, p.count)).collect();
        assert_eq!(series.len(), 3);
        assert!(days.contains(&(InvocationKind::Slash, 1)));
        assert!(series.windows(2).all(|w| w[0].day <= w[1].day));
    }

    #[test]
    fn tracking_since_is_the_earliest_event_per_harness() {
        let f = Fixture::new();
        write_lines(
            &f.transcript("p", "a.jsonl"),
            &[tool_use("t1", "x", "2026-11-03T00:00:00.000Z"), tool_use("t2", "x", "2026-10-01T12:00:00.000Z")],
        );
        let mut index = f.index();
        assert!(index.tracking_since().unwrap().is_empty());

        f.ingest(&mut index);

        let since = index.tracking_since().unwrap();
        assert_eq!(since.len(), 1);
        assert_eq!(since[&HarnessId::CLAUDE_CODE].to_rfc3339(), "2026-10-01T12:00:00+00:00");
    }

    #[test]
    fn known_projects_walk_up_to_the_git_root_and_skip_missing_folders() {
        let f = Fixture::new();
        let repo = f.home.path().join("repo");
        let sub = repo.join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        let gone = f.home.path().join("gone");
        let line = |id: &str, cwd: &Path| {
            json!({"type": "assistant", "uuid": id, "sessionId": "s", "cwd": cwd, "timestamp": T0,
                "message": {"content": [{"type": "tool_use", "id": id, "name": "Skill", "input": {"skill": "x"}}]}})
            .to_string()
        };
        write_lines(&f.transcript("p", "a.jsonl"), &[line("t1", &sub), line("t2", &repo), line("t3", &gone)]);
        let mut index = f.index();
        f.ingest(&mut index);

        let roots = index.known_projects().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0], repo);
    }

    #[test]
    fn data_survives_reopening_the_database() {
        let f = Fixture::new();
        write_lines(&f.transcript("p", "a.jsonl"), &[tool_use("t1", "x", T0)]);
        {
            let mut index = f.index();
            f.ingest(&mut index);
        }
        let mut index = f.index();
        assert_eq!(count(&index, "x"), 1);
        assert_eq!(f.ingest(&mut index).files_read, 0);
    }
}
