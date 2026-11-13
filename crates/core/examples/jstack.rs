//! Reads the jstack clone and this machine's scan, then prints the repo state, every relation with
//! its drift, what each harness lacks, and the command lines skilltab would offer. Nothing is run.
//! Only read-only git queries execute. `--fetch` adds one `git fetch`. A path argument replaces ~/jstack.

use skilltab_core::*;
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fetch = args.iter().any(|a| a == "--fetch");
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from) else {
        eprintln!("USERPROFILE is not set");
        std::process::exit(2);
    };
    let root = args.iter().find(|a| !a.starts_with("--")).map(PathBuf::from).unwrap_or_else(|| home.join("jstack"));

    if fetch {
        if let Err(e) = jstack_fetch(&root) {
            eprintln!("fetch failed: {e}");
            std::process::exit(1);
        }
    }
    let repo = match RepoState::load(&root) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read the jstack clone: {e}");
            std::process::exit(1);
        }
    };
    let env = Env { home, projects: Vec::new(), jstack_root: Some(root) };
    let snap = match Skilltab::open(env, std::env::temp_dir().join("skilltab-jstack-example")) {
        Ok(app) => app.snapshot(),
        Err(e) => {
            eprintln!("scan failed: {e:?}");
            std::process::exit(1);
        }
    };
    let rep = jstack_report(&snap, &repo);

    print_repo(&repo, fetch);
    print_relations(&snap, &rep);
    print_gaps(&snap, &rep);
    print_commands(&repo, &rep);
}

fn print_repo(repo: &RepoState, fetched_now: bool) {
    println!("RepoState");
    println!("  root      {}", repo.root.display());
    println!("  remote    {}", repo.remote.as_deref().unwrap_or("(none)"));
    println!("  head      {}", repo.head);
    println!("  branch    {}", repo.branch.as_deref().unwrap_or("(detached)"));
    match &repo.upstream {
        Some(u) => println!("  upstream  {} ahead {} behind {}", u.name, u.ahead, u.behind),
        None => println!("  upstream  (none known)"),
    }
    let fetched = repo.fetched_at.map_or("never".to_owned(), |t| t.to_rfc3339());
    println!("  fetched   {fetched}{}", if fetched_now { " (just now, --fetch)" } else { "" });
    if repo.dirty.is_empty() {
        println!("  dirty     none under skills/");
    } else {
        println!("  dirty     {} path(s) under skills/", repo.dirty.len());
        for p in &repo.dirty {
            println!("            {}", p.display());
        }
    }
    let names: Vec<&str> = repo.skills.iter().map(|s| s.name.as_str()).collect();
    println!("  skills    {} own: {}", names.len(), names.join(", "));
    println!("  declared  {} line(s), {} skill(s)", repo.declared.len(), repo.declared.iter().map(|d| d.skills.len()).sum::<usize>());
    println!("  excluded  {}", if repo.excluded.is_empty() { "none".to_owned() } else { repo.excluded.join(", ") });
}

fn print_relations(snap: &Snapshot, rep: &JstackReport) {
    println!("\nRelations ({})", rep.relations.len());
    for (id, rel) in &rep.relations {
        let Some(body) = snap.bodies.get(id) else { continue };
        let mut places: Vec<String> = body.mounts.iter().filter_map(|m| snap.mounts.get(m)).map(|m| m.harness.as_str().to_owned()).collect();
        places.sort();
        let (kind, skill, drift) = match rel {
            JstackRelation::Owned { skill, drift } => ("Owned", skill.as_str(), describe(drift)),
            JstackRelation::CopyOf { skill, drift } => ("CopyOf", skill.as_str(), describe(drift)),
            JstackRelation::Declared { skill, source } => ("Declared", skill.as_str(), format!("installed, from {source}")),
        };
        println!("  {kind:<8} {skill:<32} {drift:<40} [{}] {}", places.join(", "), body.dir.as_path().display());
    }
}

fn describe(d: &Drift) -> String {
    match d {
        Drift::InSync => "in sync".to_owned(),
        Drift::UncommittedEdits { files } => format!("uncommitted edits: {}", files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>().join(", ")),
        Drift::Behind { commits } => format!("behind upstream by {commits}"),
        Drift::Diverged { ahead, behind } => format!("diverged (ahead {ahead}, behind {behind})"),
        Drift::ContentDiffers { installed, repo } => format!("content differs ({} vs repo {})", short(&installed.0), short(&repo.0)),
    }
}

fn short(hash: &[u8; 20]) -> String {
    hash.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

fn print_gaps(snap: &Snapshot, rep: &JstackReport) {
    println!("\nRepo skills not installed, per harness");
    for h in snap.harnesses.iter().filter(|h| h.present) {
        let gaps: Vec<&NotInstalled> = rep.not_installed.iter().filter(|n| n.harness == h.id).collect();
        println!("  {} ({})", h.label, gaps.len());
        for g in gaps {
            match &g.why {
                Absence::Absent => println!("    {:<32} absent, sync would link it", g.skill),
                Absence::NameTaken { by } => {
                    let held = snap.bodies.get(by).map_or("?".to_owned(), |b| b.dir.as_path().display().to_string());
                    println!("    {:<32} name taken by {held}, sync skips it", g.skill);
                }
            }
        }
    }
    println!("\nDeclared in skill-sources.txt but not installed by the skills CLI ({})", rep.declared_missing.len());
    for m in &rep.declared_missing {
        println!("    {:<32} from {}", m.skill, m.source);
    }
}

fn print_commands(repo: &RepoState, rep: &JstackReport) {
    println!("\nCommands skilltab would offer (printed, never run here)");
    show("install or re-link", &repo.command(&JstackAction::Sync));
    show("update", &repo.command(&JstackAction::Pull));
    let mut offered = false;
    for d in &repo.declared {
        let missing: Vec<String> =
            d.skills.iter().filter(|s| rep.declared_missing.iter().any(|m| m.source == d.source && &m.skill == *s)).cloned().collect();
        if !missing.is_empty() {
            offered = true;
            show("install declared", &repo.command(&JstackAction::InstallDeclared { source: d.source.clone(), skills: missing }));
        }
    }
    if let (false, Some(d)) = (offered, repo.declared.first()) {
        let action = JstackAction::InstallDeclared { source: d.source.clone(), skills: d.skills.clone() };
        show("install declared (nothing is missing, so this is only the first line as install-tools.ps1 would run it)", &repo.command(&action));
    }
}

fn show(label: &str, c: &JstackCommand) {
    let line: Vec<String> = std::iter::once(&c.command.program).chain(&c.command.args).map(|a| quote(a)).collect();
    let cwd = c.command.cwd.as_deref().map_or("(inherit)".to_owned(), |p: &Path| p.display().to_string());
    println!("  {label}{}", if c.updates_clone { "  [pulls the clone first]" } else { "" });
    println!("    {}", line.join(" "));
    println!("    cwd {cwd}");
}

fn quote(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '\t']) { format!("\"{arg}\"") } else { arg.to_owned() }
}
