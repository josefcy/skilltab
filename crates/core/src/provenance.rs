//! Answers "where did this folder come from" and "who made this link", from the record
//! files other tools keep. skilltab reads these files and never writes them.
//!   ~/.agents/.skill-lock.json (v3)    skills CLI installs
//!   ~/.claude/skills/synced/*/manifest.json
//! Plugin and harness-builtin facts come from the harness's own discovery.

use crate::fsutil;
use crate::model::*;
use crate::Env;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

pub struct Provenance {
    lock: HashMap<String, LockEntry>,
    /// Every lock entry, with the source that installs it again. One without a source reads as None.
    install: BTreeMap<String, Option<String>>,
    synced: HashMap<String, String>,
    agents_skills: String,
    claude_skills: String,
    issues: Vec<ScanIssue>,
    /// Set when the lock exists but cannot be read as v3. A setup export then has no sources to offer.
    lock_error: Option<String>,
}

struct LockEntry {
    source: String,
    source_url: Option<String>,
    git_ref: Option<String>,
    skill_folder_hash: String,
}

/// The source `npx skills add` is given to install a skill again, chosen as `backup.ps1` chooses it. PowerShell
/// compares strings without regard to case, and an absent type is not "github". None when nothing names a source.
fn install_source(source: Option<&str>, kind: Option<&str>, url: Option<&str>, base_url: Option<&str>) -> Option<String> {
    fn set(v: Option<&str>) -> Option<&str> {
        v.filter(|s| !s.is_empty())
    }
    let is = |wanted: &str| kind.is_some_and(|k| k.eq_ignore_ascii_case(wanted));
    if is("well-known")
        && let Some(base) = set(base_url)
    {
        Some(base.to_owned())
    } else if !is("github")
        && let Some(url) = set(url)
    {
        Some(url.to_owned())
    } else {
        source.map(str::to_owned)
    }
}

/// The lock's skills grouped for a manifest, and the ones no source can install again.
#[derive(Debug)]
pub(crate) struct SkillSources {
    pub by_source: BTreeMap<Arg, BTreeSet<FolderName>>,
    /// Skills whose lock entry has no source and no url, so a restore cannot install them. Sorted.
    pub unrestorable: Vec<String>,
}

const LOCK_VERSION: i64 = 3;

impl Provenance {
    /// A missing or unreadable file means "no facts from it". It never fails the scan.
    /// A lock version other than 3 is read as no facts, and a ScanIssue is raised.
    pub fn load(env: &Env) -> Provenance {
        let agents_dir = env.home.join(".agents");
        let claude_skills = env.home.join(".claude").join("skills");
        let mut p = Provenance {
            lock: HashMap::new(),
            install: BTreeMap::new(),
            synced: HashMap::new(),
            agents_skills: norm_key(&agents_dir.join("skills")),
            claude_skills: norm_key(&claude_skills),
            issues: Vec::new(),
            lock_error: None,
        };
        p.load_lock(&agents_dir.join(".skill-lock.json"));
        p.load_synced(&claude_skills.join("synced"));
        p
    }

    pub fn issues(&self) -> &[ScanIssue] {
        &self.issues
    }

    /// The lock's skills grouped by the source that installs them again, as `backup.ps1` groups them.
    /// Err when the lock exists but is unreadable or not v3, so Export never writes a manifest from half a machine.
    /// No lock is no sources.
    pub(crate) fn skill_sources(&self) -> Result<SkillSources, String> {
        if let Some(e) = &self.lock_error {
            return Err(e.clone());
        }
        // The script keys a case-insensitive hashtable, so two spellings are one source and the first one stays.
        // Skills come by name because a parsed lock does not keep the order of its keys.
        let mut groups: BTreeMap<String, (Arg, BTreeSet<FolderName>)> = BTreeMap::new();
        let mut unrestorable = Vec::new();
        for (name, source) in &self.install {
            let Some(source) = source else {
                unrestorable.push(name.clone());
                continue;
            };
            let arg = Arg::parse(source).map_err(|e| format!("The source of skill {name} is not usable: {e}."))?;
            let skill = FolderName::parse(name).map_err(|e| format!("A skill name in the skill lock is not usable: {e}."))?;
            groups.entry(source.to_lowercase()).or_insert_with(|| (arg, BTreeSet::new())).1.insert(skill);
        }
        Ok(SkillSources { by_source: groups.into_values().collect(), unrestorable })
    }

    /// Order matters. The first match wins:
    /// synced manifest > plugin (from the harness fact) >
    /// skills lock (by folder name under ~/.agents/skills) > Unknown.
    pub fn origin(&self, dir: &RealDir, harness_fact: Option<&Origin>) -> Origin {
        let key = norm_key(dir.as_path());
        if let Some(skill_id) = self.synced.get(&key) {
            return Origin::ClaudeAiSync { skill_id: skill_id.clone() };
        }
        if let Some(fact) = harness_fact {
            return fact.clone();
        }
        if let Some(entry) = self.lock_entry_for(dir.as_path(), &self.agents_skills) {
            return Origin::SkillsCli {
                source: entry.source.clone(),
                source_url: entry.source_url.clone(),
                git_ref: entry.git_ref.clone(),
                lock_hash: entry.skill_folder_hash.clone(),
            };
        }
        Origin::Unknown
    }

    /// Only refines an Unmanaged hint. The harness's own facts (PluginManager, builtin) win.
    pub fn owner(&self, entry: &Path, hint: Ownership) -> Ownership {
        if hint != Ownership::Unmanaged {
            return hint;
        }
        if self.lock_entry_for(entry, &self.agents_skills).is_some()
            || self.lock_entry_for(entry, &self.claude_skills).is_some()
        {
            return Ownership::SkillsCli;
        }
        Ownership::Unmanaged
    }

    fn lock_entry_for(&self, path: &Path, parent_key: &str) -> Option<&LockEntry> {
        let name = path.file_name()?.to_str()?;
        let parent = norm_key(path.parent()?);
        if parent == parent_key { self.lock.get(name) } else { None }
    }

    fn problem(&mut self, path: &Path, error: String) {
        self.issues.push(ScanIssue::Unreadable { path: path.to_path_buf(), error });
    }

    fn lock_problem(&mut self, path: &Path, error: String) {
        self.lock_error = Some(format!("The skill lock {} cannot be read: {error}.", path.display()));
        self.problem(path, error);
    }

    fn load_lock(&mut self, path: &Path) {
        let json = match fsutil::read_json(path) {
            Ok(Some(j)) => j,
            Ok(None) => return,
            Err(e) => return self.lock_problem(path, e),
        };
        let version = json.get("version").and_then(|v| v.as_i64());
        if version != Some(LOCK_VERSION) {
            return self.lock_problem(path, format!("unsupported skill-lock version {version:?}, expected {LOCK_VERSION}"));
        }
        let Some(skills) = json.get("skills").and_then(|s| s.as_object()) else { return };
        for (name, v) in skills {
            let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
            let (source, source_url) = (text("source"), text("sourceUrl"));
            let install = install_source(source.as_deref(), text("sourceType").as_deref(), source_url.as_deref(), text("sourceBaseUrl").as_deref());
            self.install.insert(name.clone(), install);
            let Some(source) = source else { continue };
            self.lock.insert(
                name.clone(),
                LockEntry { source, source_url, git_ref: text("ref"), skill_folder_hash: text("skillFolderHash").unwrap_or_default() },
            );
        }
    }

    fn load_synced(&mut self, synced: &Path) {
        let dirs = match std::fs::read_dir(synced) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => return self.problem(synced, e.to_string()),
        };
        for d in dirs.flatten() {
            let manifest_path = d.path().join("manifest.json");
            let manifest = match fsutil::read_json(&manifest_path) {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(e) => {
                    self.problem(&manifest_path, e);
                    continue;
                }
            };
            let Some(skills) = manifest.get("skills").and_then(|s| s.as_array()) else { continue };
            for s in skills {
                let Some(id) = s.get("skillId").and_then(|v| v.as_str()) else { continue };
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or(id);
                for folder in [id, name] {
                    self.synced.insert(norm_key(&d.path().join(folder)), id.to_owned());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(home: &Path) -> Env {
        Env { home: home.to_path_buf(), projects: vec![] }
    }

    #[test]
    fn lock_with_wrong_version_gives_no_facts_and_an_issue() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".agents")).unwrap();
        fs::write(
            t.path().join(".agents").join(".skill-lock.json"),
            r#"{"version":2,"skills":{"a":{"source":"x/y"}}}"#,
        )
        .unwrap();
        let p = Provenance::load(&env(t.path()));
        let entry = t.path().join(".agents").join("skills").join("a");
        assert_eq!(p.owner(&entry, Ownership::Unmanaged), Ownership::Unmanaged);
        assert_eq!(p.issues().len(), 1);
    }

    #[test]
    fn lock_v3_entry_makes_skills_cli_owner() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".agents")).unwrap();
        fs::write(
            t.path().join(".agents").join(".skill-lock.json"),
            r#"{"version":3,"skills":{"a":{"source":"x/y","sourceUrl":"https://u","skillFolderHash":"h"}}}"#,
        )
        .unwrap();
        let p = Provenance::load(&env(t.path()));
        let entry = t.path().join(".agents").join("skills").join("a");
        assert_eq!(p.owner(&entry, Ownership::Unmanaged), Ownership::SkillsCli);
    }

    fn lock(home: &Path, text: &str) -> Provenance {
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(home.join(".agents").join(".skill-lock.json"), text).unwrap();
        Provenance::load(&env(home))
    }

    fn grouped(p: &Provenance) -> Vec<(String, Vec<String>)> {
        let sources = p.skill_sources().unwrap().by_source;
        sources.iter().map(|(k, v)| (k.as_str().to_owned(), v.iter().map(|n| n.as_str().to_owned()).collect())).collect()
    }

    #[test]
    fn sources_group_by_the_rule_backup_ps1_uses() {
        let t = tempfile::tempdir().unwrap();
        let p = lock(
            t.path(),
            r#"{"version":3,"skills":{
                "s-two":{"source":"acme/skills","sourceType":"github","sourceUrl":"https://github.com/acme/skills.git"},
                "s-one":{"source":"acme/skills","sourceType":"GitHub","sourceUrl":"https://github.com/acme/skills.git"},
                "Upper":{"source":"acme/skills","sourceType":"github"},
                "wk":{"source":"example.test","sourceType":"well-known","sourceBaseUrl":"https://example.test","sourceUrl":"https://example.test/x"},
                "wk-bare":{"source":"example.test","sourceType":"well-known","sourceUrl":"https://example.test/y"},
                "gitty":{"source":"ignored","sourceType":"git","sourceUrl":"https://example.test/g.git"},
                "untyped":{"source":"plain/src","sourceUrl":"https://example.test/u.git"},
                "empty-url":{"source":"plain/empty","sourceType":"git","sourceUrl":""},
                "nosource":{"sourceType":"git","sourceUrl":"https://example.test/skipped.git"},
                "nosource-wk":{"sourceType":"well-known","sourceBaseUrl":"https://example.test"}}}"#,
        );
        let group = |source: &str, names: &[&str]| (source.to_owned(), names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>());
        assert_eq!(
            grouped(&p),
            [
                group("acme/skills", &["Upper", "s-one", "s-two"]),
                group("https://example.test", &["nosource-wk", "wk"]),
                group("https://example.test/g.git", &["gitty"]),
                group("https://example.test/skipped.git", &["nosource"]),
                group("https://example.test/u.git", &["untyped"]),
                group("https://example.test/y", &["wk-bare"]),
                group("plain/empty", &["empty-url"]),
            ]
        );
    }

    #[test]
    fn sources_that_differ_only_in_case_are_one_source_named_as_the_first_skill_spells_it() {
        let t = tempfile::tempdir().unwrap();
        let spelled = |first: &str, second: &str| {
            let text = format!(
                r#"{{"version":3,"skills":{{"a-skill":{{"source":"{first}","sourceType":"github"}},"b-skill":{{"source":"{second}","sourceType":"github"}},"c-skill":{{"source":"other/src","sourceType":"github"}}}}}}"#
            );
            grouped(&lock(t.path(), &text))
        };
        let group = |source: &str, names: &[&str]| (source.to_owned(), names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>());
        assert_eq!(spelled("Acme/Skills", "acme/skills"), [group("Acme/Skills", &["a-skill", "b-skill"]), group("other/src", &["c-skill"])]);
        assert_eq!(spelled("acme/skills", "ACME/SKILLS"), [group("acme/skills", &["a-skill", "b-skill"]), group("other/src", &["c-skill"])]);
    }

    #[test]
    fn a_lock_entry_with_no_usable_source_is_named_not_dropped() {
        let t = tempfile::tempdir().unwrap();
        let p = lock(
            t.path(),
            r#"{"version":3,"skills":{
                "kept":{"source":"acme/skills","sourceType":"github"},
                "nothing":{},
                "github-url-only":{"sourceType":"github","sourceUrl":"https://example.test/x.git"},
                "empty-url":{"sourceType":"git","sourceUrl":""},
                "wk-no-base":{"sourceType":"well-known","sourceUrl":""}}}"#,
        );
        let sources = p.skill_sources().unwrap();
        assert_eq!(sources.unrestorable, ["empty-url", "github-url-only", "nothing", "wk-no-base"]);
        assert_eq!(grouped(&p), [("acme/skills".to_owned(), vec!["kept".to_owned()])]);
    }

    #[test]
    fn no_lock_is_no_sources_but_a_lock_that_cannot_be_read_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert!(Provenance::load(&env(t.path())).skill_sources().unwrap().by_source.is_empty());
        for text in [r#"{"version":2,"skills":{"a":{"source":"x/y"}}}"#, "{not json", r#"{"skills":{}}"#] {
            let e = lock(t.path(), text).skill_sources().unwrap_err();
            assert!(e.contains(".skill-lock.json"), "{e}");
        }
    }

    #[test]
    fn a_lock_entry_that_would_become_a_flag_or_a_path_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert!(lock(t.path(), r#"{"version":3,"skills":{"a":{"source":"-y"}}}"#).skill_sources().is_err());
        assert!(lock(t.path(), r#"{"version":3,"skills":{"..":{"source":"x/y"}}}"#).skill_sources().is_err());
    }
}
