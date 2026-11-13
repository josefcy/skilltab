//! Ingests this machine's transcripts into a usage database and prints what the index knows.
//! Usage: cargo run --release -p skilltab-core --example usage -- --db <path>
//! Run it twice on the same path: the second run should add no events and be much faster.

use skilltab_core::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

const TOP_SKILLS: usize = 25;
const TOP_GHOSTS: usize = 20;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let db = args
        .windows(2)
        .find(|w| w[0] == "--db")
        .map(|w| PathBuf::from(&w[1]))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("skilltab").join("usage.db")));
    let (Some(db), Some(home)) = (db, std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from)) else {
        eprintln!("usage: usage --db <path>   (USERPROFILE must be set)");
        std::process::exit(2);
    };

    if let Err(e) = run(&db, &home) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(db: &Path, home: &Path) -> Result<(), UsageError> {
    let mut env = Env { home: home.to_path_buf(), projects: Vec::new(), jstack_root: Some(home.join("jstack")) };
    let mut index = UsageIndex::open(db)?;

    let started = Instant::now();
    let report = index.ingest_all(&env, &mut |_| {})?;
    let ingest_ms = started.elapsed().as_secs_f64() * 1000.0;

    println!("ingest");
    println!("  sources seen: {}    read: {}    reset: {}", report.sources_seen, report.files_read, report.sources_reset);
    println!("  bytes read: {:.1} MB", report.bytes_read as f64 / 1_048_576.0);
    println!("  events added: {}    events stored: {}", report.events_added, index.event_count()?);
    println!("  time: {ingest_ms:.1} ms");
    for (path, err) in &report.errors {
        println!("  could not read {}: {err}", tilde(path, home));
    }

    // Projects come from the cwd of the sessions that used a skill, as the design says.
    env.projects = index.known_projects()?.into_iter().filter(|p| p != home).collect();
    let scan_env = Env { home: env.home.clone(), projects: env.projects.clone(), jstack_root: env.jstack_root.clone() };
    let scratch = std::env::temp_dir().join("skilltab-usage-example");
    let snap = match Skilltab::open(scan_env, scratch) {
        Ok(app) => app.snapshot(),
        Err(e) => {
            eprintln!("scan failed: {e:?}");
            std::process::exit(1);
        }
    };
    println!("\nsnapshot: {} bodies, {} mounts, {} project roots checked", snap.bodies.len(), snap.mounts.len(), env.projects.len());

    println!("\ntracking since (earliest event per harness)");
    let since: BTreeMap<&str, String> =
        index.tracking_since()?.into_iter().map(|(h, t)| (h.as_str(), t.format("%Y-%m-%d %H:%M UTC").to_string())).collect();
    for h in &snap.harnesses {
        println!("  {:<12} {}", h.id.as_str(), since.get(h.id.as_str()).map_or("no events", String::as_str));
    }

    let totals = index.totals(chrono::Utc::now())?;
    let mut by_body: HashMap<SkillId, Row> = HashMap::new();
    let mut ghosts: Vec<(HarnessId, InvocationKey, Tally)> = Vec::new();
    for ((harness, key), tally) in totals {
        match snap.resolve(harness, &key) {
            Some(id) => by_body.entry(id.clone()).or_default().add(harness, &tally),
            None => ghosts.push((harness, key, tally)),
        }
    }

    let kinds: Vec<&str> = InvocationKind::ALL.iter().map(|k| k.label()).collect();
    println!("\ntop {TOP_SKILLS} skills of {} used (total, last 30 days, then {})", by_body.len(), kinds.join(" / "));
    let mut rows: Vec<(&SkillId, &Row)> = by_body.iter().collect();
    rows.sort_by(|a, b| b.1.tally.total.cmp(&a.1.tally.total).then(a.0.cmp(b.0)));
    for (id, row) in rows.into_iter().take(TOP_SKILLS) {
        let Some(body) = snap.bodies.get(id) else { continue };
        println!(
            "  {:>4} {:>4}  {:<9}  {:<40} {}",
            row.tally.total,
            row.tally.last_30d,
            kind_counts(&row.tally),
            truncate(&body.manifest.name, 40),
            tilde(body.dir.as_path(), home),
        );
        println!("       {}  last {}", row.harnesses.join("+"), row.tally.last_used.map_or("-".into(), |t| t.format("%Y-%m-%d").to_string()));
    }

    ghosts.sort_by(|a, b| b.2.total.cmp(&a.2.total).then_with(|| format!("{:?}", a.1).cmp(&format!("{:?}", b.1))));
    let ghost_events: u32 = ghosts.iter().map(|g| g.2.total).sum();
    println!("\nghosts: {} unresolved keys, {ghost_events} events (top {TOP_GHOSTS})", ghosts.len());
    for (harness, key, tally) in ghosts.iter().take(TOP_GHOSTS) {
        let shown = match key {
            InvocationKey::Name(n) => n.clone(),
            InvocationKey::SkillFile(p) => tilde(p, home),
        };
        println!("  {:>4}  {:<9}  {:<12} {}", tally.total, kind_counts(tally), harness.as_str(), shown);
    }
    Ok(())
}

#[derive(Default)]
struct Row {
    tally: Tally,
    harnesses: Vec<&'static str>,
}

impl Row {
    fn add(&mut self, harness: HarnessId, t: &Tally) {
        self.tally.total += t.total;
        self.tally.last_30d += t.last_30d;
        self.tally.last_used = self.tally.last_used.max(t.last_used);
        for (a, b) in self.tally.by_kind.iter_mut().zip(t.by_kind) {
            *a += b;
        }
        if !self.harnesses.contains(&harness.as_str()) {
            self.harnesses.push(harness.as_str());
            self.harnesses.sort();
        }
    }
}

fn kind_counts(t: &Tally) -> String {
    t.by_kind.iter().map(u32::to_string).collect::<Vec<_>>().join("/")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_owned() } else { s.chars().take(max - 1).collect::<String>() + "~" }
}

fn tilde(p: &Path, home: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let h = home.to_string_lossy();
    if s.len() >= h.len() && s.is_char_boundary(h.len()) && s[..h.len()].eq_ignore_ascii_case(&h) {
        format!("~{}", &s[h.len()..])
    } else {
        s
    }
}
