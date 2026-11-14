//! Fixtures shared by the tests of every module.

use crate::fsutil::strip_verbatim;
use crate::harness::registry;
use crate::manage::Shelf;
use crate::model::Snapshot;
use crate::provenance::Provenance;
use crate::Env;
use std::collections::BTreeMap;
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

/// Gives a file or folder the Hidden attribute. Elsewhere only a leading dot hides, so this does nothing.
pub(crate) fn hide(path: &Path) {
    #[cfg(windows)]
    assert!(std::process::Command::new("attrib").arg("+h").arg(path).status().unwrap().success());
    #[cfg(unix)]
    let _ = path;
}

/// Copies the files and folders under `from` into `to`, byte for byte.
pub(crate) fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for item in fs::read_dir(from).unwrap() {
        let item = item.unwrap();
        let target = to.join(item.file_name());
        if item.file_type().unwrap().is_dir() {
            copy_tree(&item.path(), &target);
        } else {
            fs::copy(item.path(), target).unwrap();
        }
    }
}

/// Every entry under `root` without following links: a file as its text, a folder as an empty value under a key that
/// ends in `/`, and a link as `-> target`. Two listings are equal only when nothing under `root` changed.
pub(crate) fn listing(root: &Path) -> BTreeMap<String, String> {
    fn walk(dir: &Path, rel: &str, out: &mut BTreeMap<String, String>) {
        for item in fs::read_dir(dir).unwrap() {
            let path = item.unwrap().path();
            let name = format!("{rel}{}", path.file_name().unwrap().to_string_lossy());
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            if kind.is_symlink() {
                out.insert(name, format!("-> {}", fs::read_link(&path).unwrap().display()));
            } else if kind.is_dir() {
                out.insert(format!("{name}/"), String::new());
                walk(&path, &format!("{name}/"), out);
            } else {
                out.insert(name, String::from_utf8_lossy(&fs::read(&path).unwrap()).into_owned());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, "", &mut out);
    out
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
