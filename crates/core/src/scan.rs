//! Builds a Snapshot. This is the only module that turns raw paths into LinkPath, RealDir,
//! SkillId and MountId. Everything downstream trusts those types (per boundary-discipline).

mod hash;
mod manifest;
mod resolve;

use crate::harness::{Discovery, EntryFact, Harness, InstallFact, Naming, ScopeFact};
use crate::model::*;
use crate::provenance::Provenance;
use crate::Env;
use sha1::{Digest, Sha1};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(crate) use hash::content_hash;
pub(crate) use resolve::still_link_to;

/// Full rescan. About 300 entries and their SKILL.md files: it runs in milliseconds, so there is no inventory cache.
///
/// Steps:
/// 1. For each harness: `discover` gives EntryFacts and BundleFacts.
/// 2. For each entry: follow reparse points hop by hop (lstat + read_link, max 16 hops;
///    detect cycles), giving `hops: Vec<LinkPath>` and a terminal RealDir, or a ScanIssue::BrokenLink.
/// 3. Body identity = file id of the terminal dir. Entries that reach the same file id
///    share one SkillBody. This is where the 2-3 paths per skill collapse into one row.
/// 4. Per body: parse the frontmatter, compute content_hash, take the origin from provenance.
/// 5. Per mount: the owner comes from the harness hint, refined by provenance (jstack link
///    list, skills lock). Invocation keys come from the Naming and the manifest name, plus
///    SkillFile(canonical SKILL.md).
/// 6. Shadowing: within one harness, two mounts with the same Name key means the
///    harness keeps one: the lowest `EntryFact::rank`. The others become Shadowed.
/// 7. Add the shelf entries (owned by manage::Shelf) as disabled rows.
/// 8. Build SnapshotIndex.
pub fn scan(
    env: &Env,
    harnesses: &[&dyn Harness],
    projects: &[ProjectRoot],
    provenance: &Provenance,
    shelf: Vec<ShelfEntry>,
    revision: u64,
) -> Snapshot {
    let mut b = Builder::new(provenance);
    for h in harnesses {
        let discovery = h.discover(env, projects);
        b.add_harness(*h, discovery);
    }
    b.finish(shelf, revision)
}

/// Project roots from user-supplied paths. Entries that are not folders are dropped, and
/// two spellings of one path collapse into one.
pub(crate) fn project_roots(paths: &[PathBuf]) -> Vec<ProjectRoot> {
    let mut seen = HashSet::new();
    paths
        .iter()
        .filter(|p| p.is_dir())
        .filter(|p| seen.insert(norm_key(p)))
        .map(|p| ProjectRoot::mint(p.clone()))
        .collect()
}

struct Builder<'a> {
    provenance: &'a Provenance,
    harnesses: Vec<HarnessInfo>,
    bundles: HashMap<BundleId, Bundle>,
    bodies: HashMap<SkillId, SkillBody>,
    unreadable_bodies: HashSet<SkillId>,
    mounts: HashMap<MountId, Mount>,
    ranks: HashMap<MountId, u8>,
    issues: Vec<ScanIssue>,
}

impl<'a> Builder<'a> {
    fn new(provenance: &'a Provenance) -> Self {
        Builder {
            provenance,
            harnesses: Vec::new(),
            bundles: HashMap::new(),
            bodies: HashMap::new(),
            unreadable_bodies: HashSet::new(),
            mounts: HashMap::new(),
            ranks: HashMap::new(),
            issues: provenance.issues().to_vec(),
        }
    }

    fn add_harness(&mut self, h: &dyn Harness, d: Discovery) {
        let id = h.id();
        self.harnesses.push(HarnessInfo { id, label: h.label(), present: !d.entries.is_empty() || !d.bundles.is_empty() });
        for (path, error) in d.unreadable {
            self.issues.push(ScanIssue::Unreadable { path, error });
        }

        let mut bundle_ids: HashMap<String, BundleId> = HashMap::new();
        for f in d.bundles {
            let bid = BundleId::mint(format!("{}:{}", id.as_str(), f.key));
            bundle_ids.insert(f.key, bid.clone());
            let install_scope = match f.install_scope {
                InstallFact::User => InstallScope::User,
                InstallFact::Project(p) => InstallScope::Project { root: ProjectRoot::mint(p) },
            };
            self.bundles.insert(
                bid.clone(),
                Bundle {
                    id: bid,
                    harness: id,
                    name: f.name,
                    marketplace: f.marketplace,
                    version: f.version,
                    install_scope,
                    enabled: f.enabled,
                    root: f.root,
                    members: Vec::new(),
                },
            );
        }

        for fact in d.entries {
            self.add_entry(id, fact, &bundle_ids);
        }
    }

    fn add_entry(&mut self, harness: HarnessId, fact: EntryFact, bundle_ids: &HashMap<String, BundleId>) {
        let mount_id = mount_id(harness, &fact.entry);
        if self.mounts.contains_key(&mount_id) {
            return;
        }
        let (hops, dir, file_id) = match resolve::resolve_entry(&fact.entry) {
            resolve::Resolved::Body { hops, dir, file_id } => (hops, dir, file_id),
            resolve::Resolved::Broken { link, target } => {
                self.issues.push(ScanIssue::BrokenLink { id: mount_id, link, target, harness });
                return;
            }
            resolve::Resolved::Cycle => {
                self.issues.push(ScanIssue::LinkCycle { entry: fact.entry });
                return;
            }
            resolve::Resolved::Unreadable { path, error } => {
                self.issues.push(ScanIssue::Unreadable { path, error });
                return;
            }
        };

        let skill_id = skill_id(file_id);
        if self.unreadable_bodies.contains(&skill_id) {
            return;
        }
        if !self.bodies.contains_key(&skill_id) {
            match self.read_body(skill_id.clone(), dir) {
                Ok(body) => {
                    self.bodies.insert(skill_id.clone(), body);
                }
                Err((path, error)) => {
                    self.issues.push(ScanIssue::Unreadable { path, error });
                    self.unreadable_bodies.insert(skill_id);
                    return;
                }
            }
        }

        let Some((scope, bundle)) = self.scope_of(&fact.scope, harness, bundle_ids) else {
            self.issues.push(ScanIssue::Unreadable {
                path: fact.entry,
                error: "entry names a bundle its harness did not report".to_owned(),
            });
            return;
        };
        let owner = match (&bundle, &fact.owner_hint) {
            (Some(b), Ownership::Unmanaged) => Ownership::PluginManager { bundle: b.clone() },
            _ => self.provenance.owner(&fact.entry, fact.owner_hint),
        };
        let state = match &bundle {
            Some(b) if self.bundles.get(b).is_some_and(|b| !b.enabled) => MountState::BundleDisabled,
            _ => MountState::Active,
        };

        let body = &self.bodies[&skill_id];
        let mut keys = vec![InvocationKey::Name(match &fact.naming {
            Naming::Plain => body.manifest.name.clone(),
            Naming::Prefixed(prefix) => format!("{prefix}:{}", body.manifest.name),
        })];
        keys.push(InvocationKey::SkillFile(body.dir.as_path().join("SKILL.md")));

        self.ranks.insert(mount_id.clone(), fact.rank);
        self.mounts.insert(
            mount_id.clone(),
            Mount { id: mount_id, harness, scope, entry: fact.entry, hops, body: skill_id, keys, owner, state },
        );
    }

    fn scope_of(&self, s: &ScopeFact, harness: HarnessId, bundle_ids: &HashMap<String, BundleId>) -> Option<(Scope, Option<BundleId>)> {
        match s {
            ScopeFact::User => Some((Scope::User, None)),
            ScopeFact::Project(root) => Some((Scope::Project { root: root.clone() }, None)),
            ScopeFact::Bundle { key } => {
                let b = bundle_ids.get(key)?.clone();
                Some((Scope::Bundle { bundle: b.clone() }, Some(b)))
            }
        }
    }

    fn read_body(&self, id: SkillId, dir: RealDir) -> Result<SkillBody, (PathBuf, String)> {
        let skill_md = dir.as_path().join("SKILL.md");
        let bytes = std::fs::read(&skill_md).map_err(|e| (skill_md.clone(), e.to_string()))?;
        let folder = dir.as_path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let manifest = manifest::parse(&String::from_utf8_lossy(&bytes), &folder);
        let content_hash = content_hash(&dir);
        Ok(SkillBody { id, dir, manifest, content_hash, origin: Origin::Unknown, mounts: Vec::new() })
    }

    fn finish(mut self, shelf: Vec<ShelfEntry>, revision: u64) -> Snapshot {
        self.apply_shadowing();
        self.link_bodies_and_bundles();
        Snapshot::new(SnapshotParts {
            revision,
            taken_at: chrono::Utc::now(),
            harnesses: self.harnesses,
            bodies: self.bodies,
            mounts: self.mounts,
            bundles: self.bundles,
            shelf,
            issues: self.issues,
        })
    }

    /// Two mounts of one harness with the same Name key: the harness loads the lowest rank.
    /// A disabled bundle is not loaded, so it neither shadows nor is shadowed.
    fn apply_shadowing(&mut self) {
        let mut groups: HashMap<(HarnessId, &str), Vec<&MountId>> = HashMap::new();
        for m in self.mounts.values().filter(|m| m.state == MountState::Active) {
            if let Some(InvocationKey::Name(n)) = m.keys.first() {
                groups.entry((m.harness, n.as_str())).or_default().push(&m.id);
            }
        }
        let mut shadowed: Vec<(MountId, MountId)> = Vec::new();
        for ids in groups.values().filter(|g| g.len() > 1) {
            let winner = ids
                .iter()
                .min_by_key(|id| (self.ranks[**id], norm_key(&self.mounts[**id].entry)))
                .map(|w| (*w).clone());
            if let Some(winner) = winner {
                shadowed.extend(ids.iter().filter(|id| ***id != winner).map(|id| ((*id).clone(), winner.clone())));
            }
        }
        for (loser, by) in shadowed {
            if let Some(m) = self.mounts.get_mut(&loser) {
                m.state = MountState::Shadowed { by };
            }
        }
    }

    fn link_bodies_and_bundles(&mut self) {
        let mut ordered: Vec<&Mount> = self.mounts.values().collect();
        ordered.sort_by(|a, b| a.id.cmp(&b.id));

        let mut origin_fact: HashMap<SkillId, Origin> = HashMap::new();
        for m in &ordered {
            let fact = match (&m.scope, &m.owner) {
                (Scope::Bundle { bundle }, Ownership::PluginManager { .. }) => Some(Origin::Plugin { bundle: bundle.clone() }),
                (_, Ownership::HarnessBuiltin) => Some(Origin::HarnessBuiltin { harness: m.harness }),
                _ => None,
            };
            if let Some(fact) = fact {
                let keep_existing = matches!(origin_fact.get(&m.body), Some(Origin::Plugin { .. }));
                if !keep_existing {
                    origin_fact.insert(m.body.clone(), fact);
                }
            }
        }

        let mut by_body: HashMap<SkillId, Vec<MountId>> = HashMap::new();
        let mut by_bundle: HashMap<BundleId, Vec<SkillId>> = HashMap::new();
        for m in &ordered {
            by_body.entry(m.body.clone()).or_default().push(m.id.clone());
            if let Scope::Bundle { bundle } = &m.scope {
                by_bundle.entry(bundle.clone()).or_default().push(m.body.clone());
            }
        }

        for (id, body) in self.bodies.iter_mut() {
            body.mounts = by_body.remove(id).unwrap_or_default();
            body.origin = self.provenance.origin(&body.dir, origin_fact.get(id));
        }
        self.bodies.retain(|_, b| !b.mounts.is_empty());

        for (id, bundle) in self.bundles.iter_mut() {
            let mut members = by_bundle.remove(id).unwrap_or_default();
            members.sort();
            members.dedup();
            bundle.members = members;
        }
    }
}

fn skill_id((volume, index): (u32, u64)) -> SkillId {
    SkillId::mint(format!("{volume:08x}-{index:016x}"))
}

fn mount_id(harness: HarnessId, entry: &Path) -> MountId {
    let mut h = Sha1::new();
    h.update(harness.as_str().as_bytes());
    h.update([0]);
    h.update(norm_key(entry).as_bytes());
    let digest = h.finalize();
    MountId::mint(digest[..8].iter().map(|b| format!("{b:02x}")).collect())
}


#[cfg(test)]
mod tests {
    use crate::fsutil::strip_verbatim;
    use super::*;
    use crate::harness::{ParserState, SessionParser, UsageSource};
    use std::fs;
    use std::process::Command;

    struct FakeHarness {
        id: HarnessId,
        dirs: Vec<(PathBuf, u8)>,
    }

    impl Harness for FakeHarness {
        fn id(&self) -> HarnessId {
            self.id
        }
        fn label(&self) -> &'static str {
            "fake"
        }
        fn discover(&self, _env: &Env, _projects: &[ProjectRoot]) -> Discovery {
            let mut d = Discovery::default();
            for (dir, rank) in &self.dirs {
                d.scan_skill_dir(dir, ScopeFact::User, Ownership::Unmanaged, Naming::Plain, *rank);
            }
            d
        }
        fn usage_sources(&self, _env: &Env) -> Vec<UsageSource> {
            Vec::new()
        }
        fn session_parser(&self, _resume: Option<&ParserState>) -> Box<dyn SessionParser> {
            Box::new(crate::harness::NoUsage)
        }
    }

    struct Tree {
        _guard: tempfile::TempDir,
        root: PathBuf,
    }

    impl Tree {
        fn new() -> Tree {
            let guard = tempfile::tempdir().unwrap();
            let root = strip_verbatim(fs::canonicalize(guard.path()).unwrap());
            Tree { _guard: guard, root }
        }
        fn p(&self, rel: &str) -> PathBuf {
            self.root.join(rel.replace('/', "\\"))
        }
        fn skill(&self, rel: &str, name: &str) {
            let dir = self.p(rel);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nbody of {name}\n")).unwrap();
        }
        fn junction(&self, link: &str, target: &str) {
            let link = self.p(link);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            let out = Command::new("cmd").arg("/C").arg("mklink").arg("/J").arg(&link).arg(self.p(target)).output().unwrap();
            assert!(out.status.success(), "mklink failed: {}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        }
        fn env(&self) -> Env {
            Env { home: self.root.clone(), projects: vec![], jstack_root: Some(self.p("repo")) }
        }
    }

    fn run(t: &Tree, harnesses: &[&dyn Harness]) -> Snapshot {
        let env = t.env();
        let prov = Provenance::load(&env, env.jstack_root.as_deref());
        scan(&env, harnesses, &[], &prov, Vec::new(), 1)
    }

    fn claude(t: &Tree) -> FakeHarness {
        FakeHarness { id: HarnessId::CLAUDE_CODE, dirs: vec![(t.p("claude/skills"), 0)] }
    }
    fn codex(t: &Tree) -> FakeHarness {
        FakeHarness { id: HarnessId::CODEX, dirs: vec![(t.p("agents/skills"), 0)] }
    }

    fn hop_keys(m: &Mount) -> Vec<String> {
        m.hops.iter().map(|h| norm_key(h.as_path())).collect()
    }

    #[test]
    fn junction_chain_collapses_into_one_body_with_a_mount_per_harness() {
        let t = Tree::new();
        t.skill("repo/skills/x", "x");
        t.junction("agents/skills/x", "repo/skills/x");
        t.junction("claude/skills/x", "agents/skills/x");

        let snap = run(&t, &[&claude(&t), &codex(&t)]);

        assert_eq!(snap.bodies.len(), 1);
        assert_eq!(snap.mounts.len(), 2);
        assert!(snap.issues.is_empty(), "{:?}", snap.issues);

        let body = snap.bodies.values().next().unwrap();
        assert_eq!(norm_key(body.dir.as_path()), norm_key(&t.p("repo/skills/x")));
        assert_eq!(body.mounts.len(), 2);
        assert_eq!(body.origin, Origin::JstackRepo { skill: "x".into() });

        let claude_mount = snap.mounts.values().find(|m| m.harness == HarnessId::CLAUDE_CODE).unwrap();
        let codex_mount = snap.mounts.values().find(|m| m.harness == HarnessId::CODEX).unwrap();
        assert_eq!(hop_keys(claude_mount), vec![norm_key(&t.p("claude/skills/x")), norm_key(&t.p("agents/skills/x"))]);
        assert_eq!(hop_keys(codex_mount), vec![norm_key(&t.p("agents/skills/x"))]);
        assert_eq!(norm_key(&claude_mount.entry), hop_keys(claude_mount)[0]);

        let mut through_agents: Vec<_> = snap.mounts_through(&t.p("agents/skills/x")).to_vec();
        through_agents.sort();
        let mut both = vec![claude_mount.id.clone(), codex_mount.id.clone()];
        both.sort();
        assert_eq!(through_agents, both);
        assert_eq!(snap.mounts_through(&t.p("claude/skills/x")), &[claude_mount.id.clone()]);
        assert!(snap.mounts_through(&t.p("repo/skills/x")).is_empty());
        assert!(snap.mounts_through(&t.p("nowhere")).is_empty());

        let key = InvocationKey::Name("x".into());
        assert_eq!(snap.resolve(HarnessId::CLAUDE_CODE, &key), Some(&body.id));
        assert_eq!(snap.resolve(HarnessId::CODEX, &key), Some(&body.id));
        let skill_file = InvocationKey::SkillFile(body.dir.as_path().join("SKILL.md"));
        assert_eq!(snap.resolve(HarnessId::CODEX, &skill_file), Some(&body.id));
    }

    #[test]
    fn dangling_junction_is_a_broken_link_issue_and_the_rest_still_scans() {
        let t = Tree::new();
        t.skill("repo/skills/ok", "ok");
        t.junction("claude/skills/ok", "repo/skills/ok");
        t.skill("gone/y", "y");
        t.junction("claude/skills/y", "gone/y");
        fs::remove_dir_all(t.p("gone/y")).unwrap();

        let snap = run(&t, &[&claude(&t)]);

        assert_eq!(snap.mounts.len(), 1);
        assert_eq!(snap.bodies.len(), 1);
        let broken: Vec<_> = snap.issues.iter().filter_map(|i| match i {
            ScanIssue::BrokenLink { link, target, harness, id } => Some((link, target, harness, id)),
            _ => None,
        }).collect();
        assert_eq!(broken.len(), 1, "{:?}", snap.issues);
        let (link, target, harness, id) = broken[0];
        assert_eq!(norm_key(link.as_path()), norm_key(&t.p("claude/skills/y")));
        assert_eq!(norm_key(target), norm_key(&t.p("gone/y")));
        assert_eq!(*harness, HarnessId::CLAUDE_CODE);
        assert_eq!(*id, mount_id(HarnessId::CLAUDE_CODE, &t.p("claude/skills/y")));
        assert!(still_link_to(link.as_path(), &t.p("gone/y")));
        assert!(!still_link_to(link.as_path(), &t.p("elsewhere")));
    }

    #[test]
    fn broken_link_in_the_middle_of_a_chain_names_the_last_link() {
        let t = Tree::new();
        t.skill("repo/skills/z", "z");
        t.junction("agents/skills/z", "repo/skills/z");
        t.junction("claude/skills/z", "agents/skills/z");
        fs::remove_dir_all(t.p("repo/skills/z")).unwrap();

        let snap = run(&t, &[&claude(&t), &codex(&t)]);

        assert!(snap.mounts.is_empty());
        let links: Vec<_> = snap.issues.iter().filter_map(|i| match i {
            ScanIssue::BrokenLink { link, .. } => Some(norm_key(link.as_path())),
            _ => None,
        }).collect();
        assert_eq!(links, vec![norm_key(&t.p("agents/skills/z")); 2]);
    }

    #[test]
    fn link_cycle_is_reported_not_followed_forever() {
        let t = Tree::new();
        fs::create_dir_all(t.p("claude/skills/b")).unwrap();
        t.junction("claude/skills/a", "claude/skills/b");
        fs::remove_dir(t.p("claude/skills/b")).unwrap();
        t.junction("claude/skills/b", "claude/skills/a");

        let snap = run(&t, &[&claude(&t)]);

        assert!(snap.mounts.is_empty());
        assert!(snap.issues.iter().any(|i| matches!(i, ScanIssue::LinkCycle { .. })), "{:?}", snap.issues);
    }

    #[test]
    fn real_folder_entry_has_no_hops_and_two_copies_stay_two_bodies() {
        let t = Tree::new();
        t.skill("claude/skills/a", "a");
        t.skill("agents/skills/a", "a");

        let snap = run(&t, &[&claude(&t), &codex(&t)]);

        assert_eq!(snap.bodies.len(), 2);
        assert!(snap.mounts.values().all(|m| m.hops.is_empty()));
        let hashes: HashSet<_> = snap.bodies.values().map(|b| b.content_hash.clone()).collect();
        assert_eq!(hashes.len(), 1, "identical copies hash the same");
        assert_eq!(snap.mounts_through(&t.p("claude/skills/a")).len(), 1);
    }

    #[test]
    fn same_name_in_one_harness_shadows_by_rank() {
        let t = Tree::new();
        t.skill("user/skills/dup", "dup");
        t.skill("proj/skills/dup", "dup");
        let h = FakeHarness { id: HarnessId::CLAUDE_CODE, dirs: vec![(t.p("proj/skills"), 1), (t.p("user/skills"), 0)] };

        let snap = run(&t, &[&h]);

        let win = snap.mounts.values().find(|m| m.state == MountState::Active).unwrap();
        let lose = snap.mounts.values().find(|m| matches!(m.state, MountState::Shadowed { .. })).unwrap();
        assert_eq!(norm_key(&win.entry), norm_key(&t.p("user/skills/dup")));
        assert_eq!(lose.state, MountState::Shadowed { by: win.id.clone() });
        assert_eq!(snap.resolve(HarnessId::CLAUDE_CODE, &InvocationKey::Name("dup".into())), Some(&win.body));
    }

    #[test]
    fn dot_folders_and_folders_without_skill_md_are_skipped() {
        let t = Tree::new();
        t.skill("claude/skills/.trash/old", "old");
        fs::create_dir_all(t.p("claude/skills/synced/inner")).unwrap();
        t.skill("claude/skills/real", "real");

        let snap = run(&t, &[&claude(&t)]);

        assert_eq!(snap.mounts.len(), 1);
        assert!(snap.issues.is_empty());
    }

    #[test]
    fn missing_skill_root_is_not_an_error() {
        let t = Tree::new();
        let snap = run(&t, &[&claude(&t)]);
        assert!(snap.mounts.is_empty() && snap.issues.is_empty());
        assert!(!snap.harnesses[0].present);
    }

    #[test]
    fn only_scan_mints_verified_types() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offenders = Vec::new();
        let mut stack = vec![src];
        while let Some(dir) = stack.pop() {
            for item in fs::read_dir(&dir).unwrap().flatten() {
                let path = item.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path.to_string_lossy().replace('\\', "/");
                let allowed = rel.ends_with("/src/model.rs") || rel.contains("/src/scan/") || rel.ends_with("/src/scan.rs");
                if allowed || path.extension().is_none_or(|e| e != "rs") {
                    continue;
                }
                let text = fs::read_to_string(&path).unwrap();
                let needles = ["LinkPath::mint", "RealDir::mint", "SkillId::mint", "MountId::mint", "BundleId::mint", "ProjectRoot::mint"];
                if needles.iter().any(|n| text.contains(n)) {
                    offenders.push(rel);
                }
            }
        }
        assert!(offenders.is_empty(), "only scan may mint ids and verified paths: {offenders:?}");
    }
}
