//! Fixtures shared by the tests of every module.

use crate::fsutil::strip_verbatim;
use crate::harness::registry;
use crate::manage::Shelf;
use crate::model::Snapshot;
use crate::provenance::Provenance;
use crate::Env;
use std::fs;
use std::path::{Path, PathBuf};

/// Makes a directory link through the code `apply` uses: a junction on Windows, a symlink elsewhere.
pub(crate) fn link(entry: &Path, target: &Path) {
    fs::create_dir_all(entry.parent().unwrap()).unwrap();
    crate::manage::create_link(entry, target).unwrap();
}

/// Removes the link itself, whichever kind this platform makes.
pub(crate) fn unlink(entry: &Path) {
    #[cfg(windows)]
    fs::remove_dir(entry).unwrap();
    #[cfg(unix)]
    fs::remove_file(entry).unwrap();
}

pub(crate) struct Home {
    _guard: tempfile::TempDir,
    pub root: PathBuf,
}

impl Home {
    pub fn new() -> Home {
        let guard = tempfile::tempdir().unwrap();
        let root = strip_verbatim(fs::canonicalize(guard.path()).unwrap());
        Home { _guard: guard, root }
    }
    /// Joins the `/`-separated components of `rel`, so no test spells a separator of its own.
    pub fn p(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.root.clone(), |path, part| path.join(part))
    }
    pub fn skill(&self, rel: &str, name: &str) {
        let dir = self.p(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), format!("---\nname: {name}\ndescription: d\n---\nbody of {name}\n")).unwrap();
    }
    pub fn link(&self, entry: &str, target: &str) {
        link(&self.p(entry), &self.p(target));
    }
    /// A Claude plugin `pstack@cursor-plugins` with one skill, enabled in user settings.
    pub fn claude_plugin(&self) {
        let root = self.p(".claude/plugins/cache/cursor-plugins/pstack/1.0.0");
        self.skill(".claude/plugins/cache/cursor-plugins/pstack/1.0.0/skills/architect", "architect");
        let installed = serde_json::json!({"version": 2, "plugins": {"pstack@cursor-plugins": [
            {"scope": "user", "installPath": root, "version": "1.0.0"}]}});
        fs::write(self.p(".claude/plugins/installed_plugins.json"), installed.to_string()).unwrap();
        fs::write(self.p(".claude/settings.json"), r#"{"enabledPlugins":{"pstack@cursor-plugins":true}}"#).unwrap();
    }
    pub fn env(&self) -> Env {
        Env { home: self.root.clone(), projects: vec![] }
    }
    pub fn shelf(&self) -> Shelf {
        Shelf::open(&self.p(".skilltab-data")).unwrap()
    }
    pub fn scan(&self, shelf: &Shelf, revision: u64) -> Snapshot {
        let env = self.env();
        let prov = Provenance::load(&env);
        crate::scan::scan(&env, registry(), &[], &prov, shelf.entries().to_vec(), revision)
    }
}
