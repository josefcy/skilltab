//! Prints which ghost usage keys on this machine would be linked to an installed skill, and which stay unmatched.
//! Usage: cargo run -p skilltab-core --example aliases -- --data <scratch dir> [--confirm-all]
//! The scratch dir holds usage.db and aliases.json. The real %LOCALAPPDATA%\skilltab is never touched.
//! Aliases are written only with --confirm-all, and only into the scratch dir.

use skilltab_core::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let data = args.windows(2).find(|w| w[0] == "--data").map(|w| PathBuf::from(&w[1]));
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    let (Some(data), Some(home)) = (data, home) else {
        eprintln!("usage: aliases --data <scratch dir> [--confirm-all]   (USERPROFILE must be set)");
        std::process::exit(2);
    };
    if let Err(e) = run(&data, &home, args.iter().any(|a| a == "--confirm-all")) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(data: &Path, home: &Path, confirm_all: bool) -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all(data)?;
    let mut index = UsageIndex::open(&data.join("usage.db"))?;
    let mut env = Env { home: home.to_path_buf(), projects: Vec::new() };
    index.ingest_all(&env, &mut |_| {})?;
    env.projects = index.known_projects()?.into_iter().filter(|p| p != home).collect();
    let snap = Skilltab::open(env, data.to_path_buf()).map_err(|e| format!("scan failed: {e:?}"))?.snapshot();

    let store_path = data.join("aliases.json");
    let mut store = AliasStore::load(&store_path)?;
    let tallies = index.totals(chrono::Utc::now())?;

    let mut ghosts: HashMap<(HarnessId, InvocationKey), Tally> =
        tallies.iter().filter(|((h, k), _)| snap.resolve(*h, k).is_none()).map(|(k, t)| (k.clone(), *t)).collect();
    let ghost_keys = ghosts.len();
    let ghost_events: u32 = ghosts.values().map(|t| t.total).sum();
    println!("snapshot: {} bodies, {} mounts. ghosts: {ghost_keys} keys, {ghost_events} events", snap.bodies.len(), snap.mounts.len());

    let suggestions = suggest_aliases(&snap, &tallies, &store);
    let covered: u32 = suggestions.iter().map(|s| s.events).sum();
    println!("\nsuggestions: {} covering {covered} events (nothing is linked without confirmation)", suggestions.len());
    for s in &suggestions {
        let body = &snap.bodies[&s.target_body];
        println!(
            "  {:>4}  {:<11} {:<12} {}  ->  {}   [{}]",
            s.events,
            format!("{:?}", s.rule),
            s.harness.as_str(),
            shown(&s.from, home),
            shown(&s.to, home),
            tilde(body.dir.as_path(), home),
        );
        ghosts.remove(&(s.harness, s.from.clone()));
    }

    let mut left: Vec<_> = ghosts.into_iter().filter(|((h, k), _)| !store.contains_from(*h, k)).collect();
    left.sort_by(|a, b| b.1.total.cmp(&a.1.total).then_with(|| key_to_wire(&a.0.1).cmp(&key_to_wire(&b.0.1))));
    let left_events: u32 = left.iter().map(|(_, t)| t.total).sum();
    println!("\nghosts without a suggestion: {} keys, {left_events} events", left.len());
    for ((harness, key), tally) in &left {
        println!("  {:>4}  {:<12} {}", tally.total, harness.as_str(), shown(key, home));
    }

    if confirm_all {
        for s in &suggestions {
            store.add(Alias { harness: s.harness, from: s.from.clone(), to: s.to.clone() });
        }
        store.save(&store_path)?;
        let folded = apply_aliases(tallies.clone(), &store);
        let before: u32 = tallies.values().map(|t| t.total).sum();
        let after: u32 = folded.values().map(|t| t.total).sum();
        println!("\n--confirm-all wrote {} aliases to {}", store.aliases().len(), store_path.display());
        println!("events before folding {before}, after {after} (must be equal), tallied keys {} -> {}", tallies.len(), folded.len());
    }
    Ok(())
}

fn shown(key: &InvocationKey, home: &Path) -> String {
    match key {
        InvocationKey::Name(n) => n.clone(),
        InvocationKey::SkillFile(p) => tilde(p, home),
    }
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
