//! Answers "where did this folder come from" and "who made this link", from the record
//! files other tools keep. skilltab reads these files and never writes them.
//!   ~/.agents/.skill-lock.json (v3)    skills CLI installs
//!   ~/.jstack-links.txt                junctions that jstack's sync owns and recreates
//!   jstack clone path                  bodies inside it are Origin::JstackRepo
//!   ~/.claude/skills/synced/*/manifest.json
//! Plugin and harness-builtin facts come from the harness's own discovery.

use crate::fsutil;
use crate::model::*;
use crate::Env;
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub struct Provenance {
    lock: HashMap<String, LockEntry>,
    jstack_links: HashSet<String>,
    jstack_skills_dir: Option<String>,
    synced: HashMap<String, String>,
    agents_skills: String,
    claude_skills: String,
    issues: Vec<ScanIssue>,
}

struct LockEntry {
    source: String,
    source_url: Option<String>,
    git_ref: Option<String>,
    skill_folder_hash: String,
}

const LOCK_VERSION: i64 = 3;

impl Provenance {
    /// A missing or unreadable file means "no facts from it". It never fails the scan.
    /// A lock version other than 3 is read as no facts, and a ScanIssue is raised.
    pub fn load(env: &Env, jstack_root: Option<&Path>) -> Provenance {
        let agents_dir = env.home.join(".agents");
        let claude_skills = env.home.join(".claude").join("skills");
        let mut p = Provenance {
            lock: HashMap::new(),
            jstack_links: HashSet::new(),
            jstack_skills_dir: jstack_root.map(|r| {
                let skills = r.join("skills");
                norm_key(&std::fs::canonicalize(&skills).unwrap_or(skills))
            }),
            synced: HashMap::new(),
            agents_skills: norm_key(&agents_dir.join("skills")),
            claude_skills: norm_key(&claude_skills),
            issues: Vec::new(),
        };
        p.load_lock(&agents_dir.join(".skill-lock.json"));
        p.load_jstack_links(&env.home.join(".jstack-links.txt"));
        p.load_synced(&claude_skills.join("synced"));
        p
    }

    pub fn issues(&self) -> &[ScanIssue] {
        &self.issues
    }

    /// Order matters. The first match wins:
    /// inside the jstack skills dir > synced manifest > plugin (from the harness fact) >
    /// skills lock (by folder name under ~/.agents/skills) > Unknown.
    pub fn origin(&self, dir: &RealDir, harness_fact: Option<&Origin>) -> Origin {
        let key = norm_key(dir.as_path());
        if let Some(root) = &self.jstack_skills_dir {
            if let Some(rest) = key.strip_prefix(root.as_str()).and_then(|r| r.strip_prefix('\\')) {
                let skill = rest.split('\\').next().unwrap_or(rest);
                return Origin::JstackRepo { skill: skill.to_owned() };
            }
        }
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
        if self.jstack_links.contains(&norm_key(entry)) {
            return Ownership::JstackSync;
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

    fn load_lock(&mut self, path: &Path) {
        let json = match fsutil::read_json(path) {
            Ok(Some(j)) => j,
            Ok(None) => return,
            Err(e) => return self.problem(path, e),
        };
        let version = json.get("version").and_then(|v| v.as_i64());
        if version != Some(LOCK_VERSION) {
            return self.problem(path, format!("unsupported skill-lock version {version:?}, expected {LOCK_VERSION}"));
        }
        let Some(skills) = json.get("skills").and_then(|s| s.as_object()) else { return };
        for (name, v) in skills {
            let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_owned);
            let Some(source) = text("source") else { continue };
            self.lock.insert(
                name.clone(),
                LockEntry {
                    source,
                    source_url: text("sourceUrl"),
                    git_ref: text("ref"),
                    skill_folder_hash: text("skillFolderHash").unwrap_or_default(),
                },
            );
        }
    }

    fn load_jstack_links(&mut self, path: &Path) {
        match fsutil::read_text(path) {
            Ok(Some(text)) => {
                self.jstack_links = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(|l| norm_key(Path::new(l))).collect();
            }
            Ok(None) => {}
            Err(e) => self.problem(path, e.to_string()),
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
        Env { home: home.to_path_buf(), projects: vec![], jstack_root: None }
    }

    #[test]
    fn jstack_links_file_with_bom_marks_listed_entries_only() {
        let t = tempfile::tempdir().unwrap();
        let listed = t.path().join(".agents").join("skills").join("a");
        let other = t.path().join(".agents").join("skills").join("b");
        fs::write(t.path().join(".jstack-links.txt"), format!("\u{feff}{}\r\n", listed.display())).unwrap();
        let p = Provenance::load(&env(t.path()), None);
        assert_eq!(p.owner(&listed, Ownership::Unmanaged), Ownership::JstackSync);
        assert_eq!(p.owner(&other, Ownership::Unmanaged), Ownership::Unmanaged);
        assert_eq!(p.owner(&listed, Ownership::HarnessBuiltin), Ownership::HarnessBuiltin);
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
        let p = Provenance::load(&env(t.path()), None);
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
        let p = Provenance::load(&env(t.path()), None);
        let entry = t.path().join(".agents").join("skills").join("a");
        assert_eq!(p.owner(&entry, Ownership::Unmanaged), Ownership::SkillsCli);
    }
}
