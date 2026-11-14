//! Scans this machine and prints what the snapshot holds. `--json` prints the whole snapshot.
//! Project candidates come from the `cwd` field in the first lines of Claude transcripts, never from folder names.

use serde_json::{json, Value};
use skilltab_core::*;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let json_out = std::env::args().any(|a| a == "--json");
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from) else {
        eprintln!("USERPROFILE is not set");
        std::process::exit(2);
    };

    let project_count = discover_projects(&home).len();
    let env = Env { home: home.clone(), projects: Vec::new() };

    let started = Instant::now();
    let Ok(app) = Skilltab::open(env, std::env::temp_dir().join("skilltab-dump")) else { std::process::exit(1) };
    let snap = app.snapshot();
    let elapsed = started.elapsed();

    if json_out {
        println!("{}", serde_json::to_string_pretty(&snapshot_json(&snap)).unwrap_or_default());
    } else {
        summary(&snap, &home, project_count, elapsed.as_secs_f64() * 1000.0);
    }
}

// ---------------------------------------------------------------- summary

fn summary(snap: &Snapshot, home: &Path, project_count: usize, ms: f64) {
    let t = |p: &Path| tilde(p, home);

    println!("skilltab-core snapshot");
    println!("  scan time: {ms:.1} ms    project roots checked: {project_count}");
    println!("  bodies: {}    mounts: {}    bundles: {}    issues: {}", snap.bodies.len(), snap.mounts.len(), snap.bundles.len(), snap.issues.len());

    println!("\nmounts by harness and scope");
    for h in &snap.harnesses {
        let mut by_scope: BTreeMap<&str, usize> = BTreeMap::new();
        for m in snap.mounts.values().filter(|m| m.harness == h.id) {
            *by_scope.entry(scope_name(&m.scope)).or_default() += 1;
        }
        let total: usize = by_scope.values().sum();
        let parts: Vec<String> = ["user", "project", "bundle"].iter().map(|s| format!("{s} {}", by_scope.get(s).copied().unwrap_or(0))).collect();
        let present = if h.present { "" } else { "  (not present)" };
        println!("  {:<12} {:<12} total {:<4} {}{present}", h.id.as_str(), h.label, total, parts.join("  "));
    }

    println!("\nbundles");
    let mut bundles: Vec<&Bundle> = snap.bundles.values().collect();
    bundles.sort_by(|a, b| a.id.cmp(&b.id));
    for b in bundles {
        let state = if b.enabled { "enabled" } else { "disabled" };
        let version = b.version.as_deref().unwrap_or("-");
        println!("  {:<54} v{:<16} members {:<3} {state}", b.id.as_str(), version, b.members.len());
    }

    println!("\nmounts by ownership");
    count_lines(snap.mounts.values().map(|m| ownership_name(&m.owner)));
    println!("\nbodies by origin");
    count_lines(snap.bodies.values().map(|b| origin_name(&b.origin)));
    println!("\nmount states");
    count_lines(snap.mounts.values().map(|m| state_name(&m.state)));

    println!("\nbroken links");
    let mut any = false;
    for i in &snap.issues {
        if let ScanIssue::BrokenLink { link, target, harness, .. } = i {
            any = true;
            println!("  {:<12} {} -> {}", harness.as_str(), t(link.as_path()), t(target));
        }
    }
    if !any {
        println!("  none");
    }

    let shadowed: Vec<&Mount> = snap.mounts.values().filter(|m| matches!(m.state, MountState::Shadowed { .. })).collect();
    let same_body = shadowed
        .iter()
        .filter(|m| matches!(&m.state, MountState::Shadowed { by } if snap.mounts.get(by).is_some_and(|w| w.body == m.body)))
        .count();
    println!("\nshadowed mounts: {} ({} shadowed by a mount of the same body)", shadowed.len(), same_body);
    let mut sorted = shadowed;
    sorted.sort_by(|a, b| a.entry.cmp(&b.entry));
    for m in sorted {
        if let MountState::Shadowed { by } = &m.state {
            let winner = snap.mounts.get(by).map(|w| t(&w.entry)).unwrap_or_default();
            println!("  {:<12} {}  (by {})", m.harness.as_str(), t(&m.entry), winner);
        }
    }

    println!("\nname conflicts (one name, more than one body)");
    let mut by_name: BTreeMap<&str, Vec<&SkillBody>> = BTreeMap::new();
    for b in snap.bodies.values() {
        by_name.entry(b.manifest.name.as_str()).or_default().push(b);
    }
    let mut conflicts = 0;
    for (name, bodies) in by_name.iter().filter(|(_, b)| b.len() > 1) {
        conflicts += 1;
        let identical = bodies.iter().map(|b| &b.content_hash).collect::<HashSet<_>>().len() == 1;
        let note = if identical { "identical content" } else { "different content" };
        println!("  {name} ({} bodies, {note})", bodies.len());
        let mut dirs: Vec<String> = bodies
            .iter()
            .map(|b| {
                let harnesses: BTreeSet<&str> = b.mounts.iter().filter_map(|m| snap.mounts.get(m)).map(|m| m.harness.as_str()).collect();
                format!("{}  [{}]", t(b.dir.as_path()), harnesses.into_iter().collect::<Vec<_>>().join(", "))
            })
            .collect();
        dirs.sort();
        for d in dirs {
            println!("      {d}");
        }
    }
    if conflicts == 0 {
        println!("  none");
    }

    println!("\nother issues");
    let mut other = 0;
    for i in &snap.issues {
        match i {
            ScanIssue::Unreadable { path, error } => {
                other += 1;
                println!("  unreadable  {}  {error}", t(path));
            }
            ScanIssue::LinkCycle { entry } => {
                other += 1;
                println!("  cycle       {}", t(entry));
            }
            ScanIssue::BrokenLink { .. } => {}
        }
    }
    if other == 0 {
        println!("  none");
    }

    let mut manifest_issues: BTreeMap<String, usize> = BTreeMap::new();
    for b in snap.bodies.values() {
        for i in &b.manifest.issues {
            *manifest_issues.entry(manifest_issue_name(i).to_owned()).or_default() += 1;
        }
    }
    println!("\nmanifest issues (bodies)");
    if manifest_issues.is_empty() {
        println!("  none");
    }
    for (k, n) in manifest_issues {
        println!("  {k:<24} {n}");
    }
}

fn count_lines<'a>(items: impl Iterator<Item = String>) {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for i in items {
        *counts.entry(i).or_default() += 1;
    }
    for (k, n) in counts {
        println!("  {k:<24} {n}");
    }
}

fn tilde(p: &Path, home: &Path) -> String {
    let s = p.to_string_lossy().into_owned();
    let h = home.to_string_lossy();
    if s.len() >= h.len() && s[..h.len()].eq_ignore_ascii_case(&h) {
        format!("~{}", &s[h.len()..])
    } else {
        s
    }
}

fn scope_name(s: &Scope) -> &'static str {
    match s {
        Scope::User => "user",
        Scope::Project { .. } => "project",
        Scope::Bundle { .. } => "bundle",
    }
}

fn ownership_name(o: &Ownership) -> String {
    match o {
        Ownership::SkillsCli => "SkillsCli",
        Ownership::PluginManager { .. } => "PluginManager",
        Ownership::ClaudeAiSync => "ClaudeAiSync",
        Ownership::HarnessBuiltin => "HarnessBuiltin",
        Ownership::Skilltab => "Skilltab",
        Ownership::Unmanaged => "Unmanaged",
    }
    .to_owned()
}

fn origin_name(o: &Origin) -> String {
    match o {
        Origin::SkillsCli { .. } => "SkillsCli",
        Origin::Plugin { .. } => "Plugin",
        Origin::ClaudeAiSync { .. } => "ClaudeAiSync",
        Origin::HarnessBuiltin { .. } => "HarnessBuiltin",
        Origin::Unknown => "Unknown",
    }
    .to_owned()
}

fn state_name(s: &MountState) -> String {
    match s {
        MountState::Active => "Active",
        MountState::Shadowed { .. } => "Shadowed",
        MountState::BundleDisabled => "BundleDisabled",
    }
    .to_owned()
}

fn manifest_issue_name(i: &ManifestIssue) -> &'static str {
    match i {
        ManifestIssue::NoFrontmatter => "NoFrontmatter",
        ManifestIssue::MissingName => "MissingName",
        ManifestIssue::NameDiffersFromFolder { .. } => "NameDiffersFromFolder",
        ManifestIssue::MissingDescription => "MissingDescription",
    }
}

// ---------------------------------------------------------------- json

fn snapshot_json(snap: &Snapshot) -> Value {
    let path = |p: &Path| p.to_string_lossy().into_owned();

    let mut bodies: Vec<&SkillBody> = snap.bodies.values().collect();
    bodies.sort_by(|a, b| a.id.cmp(&b.id));
    let mut mounts: Vec<&Mount> = snap.mounts.values().collect();
    mounts.sort_by(|a, b| a.id.cmp(&b.id));
    let mut bundles: Vec<&Bundle> = snap.bundles.values().collect();
    bundles.sort_by(|a, b| a.id.cmp(&b.id));

    json!({
        "revision": snap.revision,
        "takenAt": snap.taken_at.to_rfc3339(),
        "harnesses": snap.harnesses.iter().map(|h| json!({"id": h.id.as_str(), "label": h.label, "present": h.present})).collect::<Vec<_>>(),
        "bodies": bodies.iter().map(|b| json!({
            "id": b.id.as_str(),
            "dir": path(b.dir.as_path()),
            "manifest": {
                "name": b.manifest.name,
                "description": b.manifest.description,
                "disableModelInvocation": b.manifest.disable_model_invocation,
                "issues": b.manifest.issues.iter().map(|i| format!("{i:?}")).collect::<Vec<_>>(),
            },
            "contentHash": b.content_hash.0.iter().map(|x| format!("{x:02x}")).collect::<String>(),
            "origin": origin_json(&b.origin),
            "mounts": b.mounts.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "mounts": mounts.iter().map(|m| json!({
            "id": m.id.as_str(),
            "harness": m.harness.as_str(),
            "scope": scope_json(&m.scope),
            "entry": path(&m.entry),
            "hops": m.hops.iter().map(|h| path(h.as_path())).collect::<Vec<_>>(),
            "body": m.body.as_str(),
            "keys": m.keys.iter().map(|k| match k {
                InvocationKey::Name(n) => json!({"name": n}),
                InvocationKey::SkillFile(p) => json!({"skillFile": path(p)}),
            }).collect::<Vec<_>>(),
            "owner": ownership_json(&m.owner),
            "state": match &m.state {
                MountState::Active => json!("active"),
                MountState::BundleDisabled => json!("bundleDisabled"),
                MountState::Shadowed { by } => json!({"shadowedBy": by.as_str()}),
            },
        })).collect::<Vec<_>>(),
        "bundles": bundles.iter().map(|b| json!({
            "id": b.id.as_str(),
            "harness": b.harness.as_str(),
            "name": b.name,
            "marketplace": b.marketplace,
            "version": b.version,
            "installScope": match &b.install_scope {
                InstallScope::User => json!("user"),
                InstallScope::Project { root } => json!({"project": path(root.path())}),
            },
            "enabled": b.enabled,
            "root": path(&b.root),
            "members": b.members.iter().map(|m| m.as_str()).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "issues": snap.issues.iter().map(|i| match i {
            ScanIssue::BrokenLink { id, link, target, harness } => json!({
                "kind": "brokenLink", "mount": id.as_str(), "harness": harness.as_str(),
                "link": path(link.as_path()), "target": path(target),
            }),
            ScanIssue::Unreadable { path: p, error } => json!({"kind": "unreadable", "path": path(p), "error": error}),
            ScanIssue::LinkCycle { entry } => json!({"kind": "linkCycle", "entry": path(entry)}),
        }).collect::<Vec<_>>(),
    })
}

fn scope_json(s: &Scope) -> Value {
    match s {
        Scope::User => json!("user"),
        Scope::Project { root } => json!({"project": root.path().to_string_lossy()}),
        Scope::Bundle { bundle } => json!({"bundle": bundle.as_str()}),
    }
}

fn ownership_json(o: &Ownership) -> Value {
    match o {
        Ownership::PluginManager { bundle } => json!({"pluginManager": bundle.as_str()}),
        other => json!(ownership_name(other)),
    }
}

fn origin_json(o: &Origin) -> Value {
    match o {
        Origin::SkillsCli { source, source_url, git_ref, lock_hash } => {
            json!({"skillsCli": {"source": source, "sourceUrl": source_url, "ref": git_ref, "lockHash": lock_hash}})
        }
        Origin::Plugin { bundle } => json!({"plugin": bundle.as_str()}),
        Origin::ClaudeAiSync { skill_id } => json!({"claudeAiSync": skill_id}),
        Origin::HarnessBuiltin { harness } => json!({"harnessBuiltin": harness.as_str()}),
        Origin::Unknown => json!("unknown"),
    }
}
