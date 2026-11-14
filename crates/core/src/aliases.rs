//! Links from usage keys that no longer resolve ("ghosts") to installed skills.
//!
//! Usage is stored under the name the harness used at the time, so a renamed or replaced plugin
//! leaves history under keys no mount answers to. This module suggests a link when the names agree
//! once the old spelling is dropped, and folds a link into the tallies only after the user confirmed
//! it (the confirmed set is `aliases.json`). Nothing here merges on its own.
//!
//! `suggest` and `apply` are pure. The store is the only part that touches the disk.

use crate::model::{HarnessId, InvocationKey, SkillId, Snapshot, Tally};
use crate::view::Tallies;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Usage under `from` counts for `to`. `to` is the installed mount's own key, which survives rescans
/// and version bumps, unlike a `SkillId` or a path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Alias {
    pub harness: HarnessId,
    pub from: InvocationKey,
    pub to: InvocationKey,
}

// ---------------------------------------------------------------- wire form of a key

/// One spelling of an `InvocationKey` for the JSON file and the IPC ids: `name:<n>` or `file:<path>`.
pub fn key_to_wire(key: &InvocationKey) -> String {
    match key {
        InvocationKey::Name(n) => format!("name:{n}"),
        InvocationKey::SkillFile(p) => format!("file:{}", p.to_string_lossy()),
    }
}

pub fn key_from_wire(s: &str) -> Option<InvocationKey> {
    if let Some(n) = s.strip_prefix("name:") {
        (!n.is_empty()).then(|| InvocationKey::Name(n.to_owned()))
    } else {
        s.strip_prefix("file:").filter(|p| !p.is_empty()).map(|p| InvocationKey::SkillFile(PathBuf::from(p)))
    }
}

// ---------------------------------------------------------------- store

#[derive(Debug)]
pub struct AliasError(String);

impl std::fmt::Display for AliasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for AliasError {}

/// Confirmed aliases, in the order they were confirmed. A `(harness, from)` pair has one target.
#[derive(Default, Clone, Debug)]
pub struct AliasStore {
    aliases: Vec<Alias>,
}

#[derive(Serialize, Deserialize)]
struct FileForm {
    version: u32,
    #[serde(default)]
    aliases: Vec<Record>,
}

#[derive(Serialize, Deserialize)]
struct Record {
    harness: String,
    from: String,
    to: String,
}

/// Serializes load-modify-save, so two confirmations cannot overwrite each other.
static WRITE: Mutex<()> = Mutex::new(());

impl AliasStore {
    pub fn aliases(&self) -> &[Alias] {
        &self.aliases
    }

    pub fn contains_from(&self, harness: HarnessId, from: &InvocationKey) -> bool {
        self.aliases.iter().any(|a| a.harness == harness && a.from == *from)
    }

    /// Returns whether the store changed. Adding the same alias again changes nothing, and a new
    /// target for an already aliased `from` replaces the old one in place.
    pub fn add(&mut self, alias: Alias) -> bool {
        if alias.from == alias.to {
            return false;
        }
        match self.aliases.iter_mut().find(|a| a.harness == alias.harness && a.from == alias.from) {
            Some(existing) if *existing == alias => false,
            Some(existing) => {
                *existing = alias;
                true
            }
            None => {
                self.aliases.push(alias);
                true
            }
        }
    }

    /// Returns whether an alias was removed.
    pub fn remove(&mut self, harness: HarnessId, from: &InvocationKey) -> bool {
        let before = self.aliases.len();
        self.aliases.retain(|a| !(a.harness == harness && a.from == *from));
        self.aliases.len() != before
    }

    /// A missing file is an empty store. A file that cannot be read or parsed is an error, so a
    /// later `save` cannot overwrite aliases the user confirmed.
    pub fn load(path: &Path) -> Result<AliasStore, AliasError> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(AliasStore::default()),
            Err(e) => return Err(AliasError(format!("cannot read {}: {e}", path.display()))),
        };
        let form: FileForm = serde_json::from_slice(&bytes)
            .map_err(|e| AliasError(format!("{} is not a valid alias file: {e}", path.display())))?;
        let aliases = form
            .aliases
            .into_iter()
            .filter_map(|r| {
                Some(Alias { harness: HarnessId::parse(&r.harness)?, from: key_from_wire(&r.from)?, to: key_from_wire(&r.to)? })
            })
            .collect();
        Ok(AliasStore { aliases })
    }

    /// Writes a temp file beside the target, then renames it over, so a crash leaves the old file whole.
    pub fn save(&self, path: &Path) -> Result<(), AliasError> {
        let form = FileForm {
            version: 1,
            aliases: self
                .aliases
                .iter()
                .map(|a| Record { harness: a.harness.as_str().to_owned(), from: key_to_wire(&a.from), to: key_to_wire(&a.to) })
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&form).map_err(|e| AliasError(e.to_string()))?;
        let io = |e: std::io::Error| AliasError(format!("cannot write {}: {e}", path.display()));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(io)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(io)?;
        std::fs::rename(&tmp, path).map_err(io)
    }

    /// Load, change, save under one lock. The file is written only when `change` reports a change.
    pub fn edit(path: &Path, change: impl FnOnce(&mut AliasStore) -> bool) -> Result<bool, AliasError> {
        let _guard = WRITE.lock().unwrap_or_else(|e| e.into_inner());
        let mut store = AliasStore::load(path)?;
        let changed = change(&mut store);
        if changed {
            store.save(path)?;
        }
        Ok(changed)
    }
}

// ---------------------------------------------------------------- suggestions

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RuleId {
    /// `old-plugin:name` becomes `name`.
    DropPrefix,
    /// `...\<folder>\SKILL.md` becomes `<folder>` (Codex reads a skill by opening its file).
    SkillFolder,
}

/// A rule reads a ghost key and names the skill it may stand for. Whether exactly one installed
/// skill of that name exists is decided once, for every rule, in `suggest`.
struct Rule {
    id: RuleId,
    candidate: fn(&InvocationKey) -> Option<String>,
}

const RULES: &[Rule] = &[Rule { id: RuleId::DropPrefix, candidate: drop_prefix }, Rule { id: RuleId::SkillFolder, candidate: skill_folder }];

fn drop_prefix(key: &InvocationKey) -> Option<String> {
    let InvocationKey::Name(n) = key else { return None };
    let (prefix, bare) = n.split_once(':')?;
    (!prefix.is_empty() && !bare.is_empty()).then(|| bare.to_owned())
}

fn skill_folder(key: &InvocationKey) -> Option<String> {
    let InvocationKey::SkillFile(p) = key else { return None };
    if !p.file_name()?.eq_ignore_ascii_case("SKILL.md") {
        return None;
    }
    p.parent()?.file_name().map(|f| f.to_string_lossy().into_owned())
}

#[derive(Clone, Debug)]
pub struct Suggestion {
    pub harness: HarnessId,
    pub from: InvocationKey,
    /// The installed mount's own name key.
    pub to: InvocationKey,
    pub target_body: SkillId,
    pub events: u32,
    pub rule: RuleId,
}

/// A ghost is a tallied key that no mount resolves. It gets a suggestion only when exactly one
/// installed body in the same harness answers to the name a rule derives from it. Zero or several
/// candidates, and keys already aliased, get none. Built-ins such as `compact` get none because no
/// mount answers to them.
pub fn suggest(snap: &Snapshot, tallies: &Tallies, store: &AliasStore) -> Vec<Suggestion> {
    let mut bodies_by_name: HashMap<(HarnessId, &str), Vec<&SkillId>> = HashMap::new();
    for m in snap.mounts.values() {
        for key in &m.keys {
            if let InvocationKey::Name(n) = key {
                let bodies = bodies_by_name.entry((m.harness, n.as_str())).or_default();
                if !bodies.contains(&&m.body) {
                    bodies.push(&m.body);
                }
            }
        }
    }

    let mut out: Vec<Suggestion> = tallies
        .iter()
        .filter(|((h, key), _)| snap.resolve(*h, key).is_none() && !store.contains_from(*h, key))
        .filter_map(|((harness, key), tally)| {
            RULES.iter().find_map(|rule| {
                let name = (rule.candidate)(key)?;
                let [body] = bodies_by_name.get(&(*harness, name.as_str()))?.as_slice() else { return None };
                let body = (*body).clone();
                Some(Suggestion {
                    harness: *harness,
                    from: key.clone(),
                    to: InvocationKey::Name(name),
                    target_body: body,
                    events: tally.total,
                    rule: rule.id,
                })
            })
        })
        .collect();
    out.sort_by(|a, b| {
        b.events.cmp(&a.events).then_with(|| a.harness.cmp(&b.harness)).then_with(|| key_to_wire(&a.from).cmp(&key_to_wire(&b.from)))
    });
    out
}

// ---------------------------------------------------------------- folding

/// Moves each aliased `from` tally into its target. An alias whose target is itself aliased follows
/// the chain, so the result does not depend on the order of the aliases. A cycle folds nothing.
pub fn apply(mut tallies: Tallies, store: &AliasStore) -> Tallies {
    let next: HashMap<(HarnessId, &InvocationKey), &InvocationKey> =
        store.aliases.iter().map(|a| ((a.harness, &a.from), &a.to)).collect();

    for alias in &store.aliases {
        let mut dest = &alias.to;
        let mut hops = 0;
        while let Some(further) = next.get(&(alias.harness, dest)) {
            dest = further;
            hops += 1;
            if hops > next.len() {
                break;
            }
        }
        if hops > next.len() {
            continue;
        }
        if let Some(moved) = tallies.remove(&(alias.harness, alias.from.clone())) {
            merge(tallies.entry((alias.harness, dest.clone())).or_default(), &moved);
        }
    }
    tallies
}

fn merge(into: &mut Tally, from: &Tally) {
    into.total += from.total;
    into.last_30d += from.last_30d;
    into.last_used = into.last_used.max(from.last_used);
    for (a, b) in into.by_kind.iter_mut().zip(from.by_kind) {
        *a += b;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::link;
    use crate::Env;
    use chrono::{TimeZone, Utc};
    use std::fs;

    fn skill(home: &Path, rel: &str, name: &str) {
        let dir = home.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: does {name}\n---\n# {name}\n")).unwrap();
    }

    fn snapshot(home: &Path) -> Snapshot {
        crate::scan_disk(&Env { home: home.to_path_buf(), projects: Vec::new() }, &[], 1)
    }

    fn name(n: &str) -> InvocationKey {
        InvocationKey::Name(n.to_owned())
    }

    fn tally(total: u32, last_30d: u32, day: Option<u32>, by_kind: [u32; 4]) -> Tally {
        Tally { total, last_30d, last_used: day.map(|d| Utc.with_ymd_and_hms(2026, 3, d, 12, 0, 0).unwrap()), by_kind }
    }

    fn tallies(entries: &[(HarnessId, InvocationKey, Tally)]) -> Tallies {
        entries.iter().map(|(h, k, t)| ((*h, k.clone()), *t)).collect()
    }

    const CLAUDE: HarnessId = HarnessId::CLAUDE_CODE;
    const CODEX: HarnessId = HarnessId::CODEX;

    #[test]
    fn prefix_ghost_maps_to_the_one_installed_skill_of_that_name() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        let snap = snapshot(tmp.path());
        let t = tallies(&[(CLAUDE, name("mattpocock-skills:tdd"), tally(7, 2, Some(5), [5, 2, 0, 0]))]);

        let s = suggest(&snap, &t, &AliasStore::default());
        assert_eq!(s.len(), 1);
        assert_eq!((s[0].harness, &s[0].from, &s[0].to), (CLAUDE, &name("mattpocock-skills:tdd"), &name("tdd")));
        assert_eq!((s[0].events, s[0].rule), (7, RuleId::DropPrefix));
        assert_eq!(snap.resolve(CLAUDE, &name("tdd")), Some(&s[0].target_body));
    }

    #[test]
    fn a_skill_in_another_harness_is_not_a_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".codex/skills/tdd", "tdd");
        let snap = snapshot(tmp.path());
        let t = tallies(&[(CLAUDE, name("old:tdd"), tally(3, 0, None, [3, 0, 0, 0]))]);
        assert!(suggest(&snap, &t, &AliasStore::default()).is_empty());
    }

    #[test]
    fn two_bodies_answering_to_the_name_make_it_ambiguous() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        skill(tmp.path(), ".claude/skills/tdd-copy", "tdd");
        let snap = snapshot(tmp.path());
        assert_eq!(snap.bodies.len(), 2);
        let t = tallies(&[(CLAUDE, name("old:tdd"), tally(3, 0, None, [3, 0, 0, 0]))]);
        assert!(suggest(&snap, &t, &AliasStore::default()).is_empty());
    }

    #[test]
    fn one_body_mounted_twice_is_still_one_candidate() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".agents/skills/tdd", "tdd");
        let real = tmp.path().join(".agents/skills/tdd");
        for folder in ["tdd", "tdd-again"] {
            let entry = tmp.path().join(".claude/skills").join(folder);
            link(&entry, &real);
        }
        let snap = snapshot(tmp.path());
        assert_eq!(snap.bodies.len(), 1);
        assert_eq!(snap.mounts.values().filter(|m| m.harness == CLAUDE).count(), 2);
        let t = tallies(&[(CLAUDE, name("old:tdd"), tally(3, 0, None, [3, 0, 0, 0]))]);
        assert_eq!(suggest(&snap, &t, &AliasStore::default()).len(), 1);
    }

    #[test]
    fn skill_file_ghost_maps_by_folder_name() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".codex/skills/code-review", "code-review");
        let snap = snapshot(tmp.path());
        let ghost = InvocationKey::SkillFile(PathBuf::from(r"C:\gone\.codex\skills\code-review\SKILL.md"));
        let t = tallies(&[(CODEX, ghost.clone(), tally(4, 1, Some(2), [0, 0, 0, 4]))]);

        let s = suggest(&snap, &t, &AliasStore::default());
        assert_eq!(s.len(), 1);
        assert_eq!((&s[0].from, &s[0].to, s[0].rule), (&ghost, &name("code-review"), RuleId::SkillFolder));

        let not_skill_md = InvocationKey::SkillFile(PathBuf::from(r"C:\gone\code-review\notes.md"));
        let t = tallies(&[(CODEX, not_skill_md, tally(1, 0, None, [0, 0, 0, 1]))]);
        assert!(suggest(&snap, &t, &AliasStore::default()).is_empty());
    }

    #[test]
    fn an_already_aliased_key_is_not_suggested_again() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        let snap = snapshot(tmp.path());
        let t = tallies(&[(CLAUDE, name("old:tdd"), tally(3, 0, None, [3, 0, 0, 0]))]);
        let mut store = AliasStore::default();
        store.add(Alias { harness: CLAUDE, from: name("old:tdd"), to: name("tdd") });
        assert!(suggest(&snap, &t, &store).is_empty());
    }

    #[test]
    fn built_in_commands_and_resolving_keys_get_no_suggestion() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/tdd", "tdd");
        let snap = snapshot(tmp.path());
        let t = tallies(&[
            (CLAUDE, name("compact"), tally(9, 1, None, [9, 0, 0, 0])),
            (CLAUDE, name("clear"), tally(2, 0, None, [2, 0, 0, 0])),
            (CLAUDE, name("tdd"), tally(5, 1, None, [5, 0, 0, 0])),
            (CLAUDE, name("a:b:"), tally(1, 0, None, [1, 0, 0, 0])),
            (CLAUDE, name(":tdd"), tally(1, 0, None, [1, 0, 0, 0])),
        ]);
        assert!(suggest(&snap, &t, &AliasStore::default()).is_empty());
    }

    #[test]
    fn suggestions_come_largest_first() {
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), ".claude/skills/a", "a");
        skill(tmp.path(), ".claude/skills/b", "b");
        let snap = snapshot(tmp.path());
        let t = tallies(&[
            (CLAUDE, name("old:a"), tally(2, 0, None, [2, 0, 0, 0])),
            (CLAUDE, name("old:b"), tally(8, 0, None, [8, 0, 0, 0])),
        ]);
        let events: Vec<u32> = suggest(&snap, &t, &AliasStore::default()).iter().map(|s| s.events).collect();
        assert_eq!(events, [8, 2]);
    }

    #[test]
    fn apply_adds_counts_and_keeps_the_latest_use_when_the_target_is_already_tallied() {
        let mut store = AliasStore::default();
        store.add(Alias { harness: CLAUDE, from: name("old:tdd"), to: name("tdd") });
        let t = tallies(&[
            (CLAUDE, name("old:tdd"), tally(7, 2, Some(9), [4, 3, 0, 0])),
            (CLAUDE, name("tdd"), tally(5, 1, Some(4), [1, 2, 1, 1])),
            (CLAUDE, name("other"), tally(1, 1, Some(1), [1, 0, 0, 0])),
            (CODEX, name("old:tdd"), tally(2, 0, Some(2), [0, 0, 2, 0])),
        ]);

        let folded = apply(t, &store);
        let tdd = folded[&(CLAUDE, name("tdd"))];
        assert_eq!((tdd.total, tdd.last_30d, tdd.by_kind), (12, 3, [5, 5, 1, 1]));
        assert_eq!(tdd.last_used, Some(Utc.with_ymd_and_hms(2026, 3, 9, 12, 0, 0).unwrap()));
        assert!(!folded.contains_key(&(CLAUDE, name("old:tdd"))));
        assert_eq!(folded[&(CLAUDE, name("other"))].total, 1);
        assert_eq!(folded[&(CODEX, name("old:tdd"))].total, 2, "the alias is per harness");
        assert_eq!(folded.values().map(|t| t.total).sum::<u32>(), 15, "no event is lost or counted twice");
    }

    #[test]
    fn apply_creates_the_target_when_it_has_no_tally_and_is_stable_when_applied_to_its_own_output() {
        let mut store = AliasStore::default();
        store.add(Alias { harness: CLAUDE, from: name("old:tdd"), to: name("tdd") });
        let t = tallies(&[(CLAUDE, name("old:tdd"), tally(3, 1, Some(3), [3, 0, 0, 0]))]);
        let once = apply(t, &store);
        assert_eq!(once[&(CLAUDE, name("tdd"))].total, 3);
        let twice = apply(once, &store);
        assert_eq!(twice[&(CLAUDE, name("tdd"))].total, 3);
    }

    #[test]
    fn apply_follows_a_chain_to_its_end_in_any_order() {
        let ab = Alias { harness: CLAUDE, from: name("a"), to: name("b") };
        let bc = Alias { harness: CLAUDE, from: name("b"), to: name("c") };
        let t = || {
            tallies(&[
                (CLAUDE, name("a"), tally(1, 0, None, [1, 0, 0, 0])),
                (CLAUDE, name("b"), tally(2, 0, None, [2, 0, 0, 0])),
                (CLAUDE, name("c"), tally(4, 0, None, [4, 0, 0, 0])),
            ])
        };
        for order in [[&ab, &bc], [&bc, &ab]] {
            let mut store = AliasStore::default();
            for a in order {
                store.add(a.clone());
            }
            let folded = apply(t(), &store);
            assert_eq!(folded.len(), 1);
            assert_eq!(folded[&(CLAUDE, name("c"))].total, 7);
        }
    }

    #[test]
    fn apply_leaves_a_cycle_alone() {
        let mut store = AliasStore::default();
        store.add(Alias { harness: CLAUDE, from: name("a"), to: name("b") });
        store.add(Alias { harness: CLAUDE, from: name("b"), to: name("a") });
        let t = tallies(&[(CLAUDE, name("a"), tally(1, 0, None, [1, 0, 0, 0])), (CLAUDE, name("b"), tally(2, 0, None, [2, 0, 0, 0]))]);
        let folded = apply(t, &store);
        assert_eq!(folded.values().map(|t| t.total).sum::<u32>(), 3);
        assert_eq!(folded.len(), 2);
    }

    #[test]
    fn the_store_round_trips_and_adding_twice_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("data").join("aliases.json");
        assert!(AliasStore::load(&path).unwrap().aliases().is_empty(), "a missing file is an empty store");

        let a = Alias { harness: CODEX, from: InvocationKey::SkillFile(PathBuf::from(r"C:\x y\code-review\SKILL.md")), to: name("code-review") };
        let b = Alias { harness: CLAUDE, from: name("old:tdd"), to: name("tdd") };
        let mut store = AliasStore::default();
        assert!(store.add(a.clone()));
        assert!(store.add(b.clone()));
        assert!(!store.add(a.clone()), "idempotent add");
        assert!(!store.add(Alias { harness: CLAUDE, from: name("same"), to: name("same") }), "no self alias");
        store.save(&path).unwrap();
        assert!(!path.with_extension("json.tmp").exists(), "the temp file is renamed away");

        let loaded = AliasStore::load(&path).unwrap();
        assert_eq!(loaded.aliases(), [a.clone(), b.clone()]);

        let mut retarget = loaded.clone();
        assert!(retarget.add(Alias { harness: CLAUDE, from: name("old:tdd"), to: name("grilling") }));
        assert_eq!(retarget.aliases().len(), 2);
        assert_eq!(retarget.aliases()[1].to, name("grilling"));

        assert!(!AliasStore::edit(&path, |s| s.add(b.clone())).unwrap(), "unchanged store is not rewritten");
        assert!(AliasStore::edit(&path, |s| s.remove(CLAUDE, &b.from)).unwrap());
        assert!(!AliasStore::edit(&path, |s| s.remove(CLAUDE, &b.from)).unwrap(), "idempotent remove");
        assert_eq!(AliasStore::load(&path).unwrap().aliases(), [a]);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_store() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("aliases.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(AliasStore::load(&path).is_err());
        assert!(AliasStore::edit(&path, |s| s.add(Alias { harness: CLAUDE, from: name("a"), to: name("b") })).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json", "a failed edit leaves the file alone");
    }

    #[test]
    fn wire_keys_round_trip() {
        for k in [name("tdd"), name("p:q"), InvocationKey::SkillFile(PathBuf::from(r"C:\a b\SKILL.md"))] {
            assert_eq!(key_from_wire(&key_to_wire(&k)), Some(k));
        }
        assert_eq!(key_from_wire("bogus"), None);
        assert_eq!(key_from_wire("name:"), None);
    }
}
