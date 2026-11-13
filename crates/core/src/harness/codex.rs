//! Codex.
//! Skills:   ~/.agents/skills/<name> (Codex loads it directly; jstack's sync treats it as Codex's skill dir),
//!           ~/.codex/skills/<name>, ~/.codex/skills/.system/<name> (HarnessBuiltin),
//!           ~/.codex/plugins/cache/<mkt>/<plugin>/<ver>/skills/<name>, named "<plugin>:<name>".
//!           A plugin is declared in config.toml as [plugins."<p>@<mkt>"] enabled = bool. A plugin
//!           installed from a remote catalog has no config.toml entry. It is marked by a
//!           .codex-remote-plugin-install.json file beside its version folders.
//!           Marketplaces openai-bundled and openai-primary-runtime ship with Codex, so their bundles are HarnessBuiltin.
//! Usage:    ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
//!           line 0: {"type":"session_meta","payload":{"id","cwd",...}}
//!           Codex has no Skill tool. A skill is "used" when a response_item custom_tool_call/function_call
//!           input names a path ending in \skills\<x>\SKILL.md (FileRead), or a user message has $<name> (Mention).
//!           The "## Skills" listing in developer messages is availability, not use, and is ignored.

use super::*;
use std::collections::BTreeMap;

pub(super) struct Codex;

const RANK_PROJECT: u8 = 0;
const RANK_AGENTS_USER: u8 = 1;
const RANK_CODEX_USER: u8 = 2;
const RANK_SYSTEM: u8 = 3;
/// A remotely installed plugin is the one Codex lists when a bundled plugin has the same name.
const RANK_REMOTE_PLUGIN: u8 = 4;
const RANK_BUILTIN_PLUGIN: u8 = 5;

const BUILTIN_MARKETPLACES: &[&str] = &["openai-bundled", "openai-primary-runtime"];
const PLUGIN_MANIFESTS: &[&str] = &[".codex-plugin/plugin.json"];
const REMOTE_MARKER: &str = ".codex-remote-plugin-install.json";

impl Harness for Codex {
    fn id(&self) -> HarnessId {
        HarnessId::CODEX
    }
    fn label(&self) -> &'static str {
        "Codex"
    }

    fn discover(&self, env: &Env, projects: &[ProjectRoot]) -> Discovery {
        let mut d = Discovery::default();
        let codex = env.home.join(".codex");

        let user = |d: &mut Discovery, dir: PathBuf, owner: Ownership, rank: u8| {
            d.scan_skill_dir(&dir, ScopeFact::User, owner, Naming::Plain, rank);
        };
        user(&mut d, env.home.join(".agents").join("skills"), Ownership::Unmanaged, RANK_AGENTS_USER);
        user(&mut d, codex.join("skills"), Ownership::Unmanaged, RANK_CODEX_USER);
        user(&mut d, codex.join("skills").join(".system"), Ownership::HarnessBuiltin, RANK_SYSTEM);

        for p in projects {
            for dir in [p.path().join(".agents").join("skills"), p.path().join(".codex").join("skills")] {
                d.scan_skill_dir(&dir, ScopeFact::Project(p.clone()), Ownership::Unmanaged, Naming::Plain, RANK_PROJECT);
            }
        }
        discover_plugins(&mut d, &codex);
        d
    }

    fn usage_sources(&self, env: &Env) -> Vec<UsageSource> {
        unimplemented!() // append_only: true
    }

    fn session_parser(&self, resume: Option<&ParserState>) -> Box<dyn SessionParser> {
        // State: (session id, cwd) from session_meta. On resume it is restored from `resume`.
        // SkillFile keys are canonicalized so that a read through a junction resolves to the same body.
        // dedup: call_id for FileRead, message id + name for Mention.
        unimplemented!()
    }

    fn bundle_command(&self, b: &BundleRef, op: BundleOp) -> Option<ExternalCommand> {
        None // no known CLI for toggling Codex plugins; the planner shows a blocker with "edit config.toml"
    }
}

fn discover_plugins(d: &mut Discovery, codex: &Path) {
    let config = read_plugin_config(d, &codex.join("config.toml"));
    let cache = codex.join("plugins").join("cache");

    for marketplace in child_dirs(d, &cache) {
        let Some(mkt) = file_name(&marketplace) else { continue };
        for plugin_dir in child_dirs(d, &marketplace) {
            let Some(name) = file_name(&plugin_dir) else { continue };
            let key = format!("{name}@{mkt}");
            let remote = plugin_dir.join(REMOTE_MARKER).is_file();
            let enabled = match config.get(&key) {
                Some(enabled) => *enabled,
                None if remote => true,
                None => continue,
            };
            let Some((version, root)) = newest_version(d, &plugin_dir) else { continue };

            let builtin = BUILTIN_MARKETPLACES.contains(&mkt.as_str());
            let (owner, rank) = if builtin {
                (Ownership::HarnessBuiltin, RANK_BUILTIN_PLUGIN)
            } else {
                (Ownership::Unmanaged, RANK_REMOTE_PLUGIN)
            };
            d.bundles.push(BundleFact {
                key: key.clone(),
                name: name.clone(),
                marketplace: mkt.clone(),
                version: Some(version),
                install_scope: InstallFact::User,
                enabled,
                root: root.clone(),
            });
            d.scan_plugin_skills(&root, PLUGIN_MANIFESTS, ScopeFact::Bundle { key }, owner, Naming::Prefixed(name), rank);
        }
    }
}

/// `[plugins."<p>@<mkt>"] enabled = bool` from config.toml. A table without `enabled` counts as enabled.
fn read_plugin_config(d: &mut Discovery, path: &Path) -> BTreeMap<String, bool> {
    let text = match fsutil::read_text(path) {
        Ok(Some(t)) => t,
        Ok(None) => return BTreeMap::new(),
        Err(e) => {
            d.unreadable.push((path.to_path_buf(), e.to_string()));
            return BTreeMap::new();
        }
    };
    let table: toml::Table = match text.parse() {
        Ok(t) => t,
        Err(e) => {
            d.unreadable.push((path.to_path_buf(), format!("invalid TOML: {e}")));
            return BTreeMap::new();
        }
    };
    let Some(plugins) = table.get("plugins").and_then(|p| p.as_table()) else { return BTreeMap::new() };
    plugins
        .iter()
        .filter_map(|(k, v)| v.as_table().map(|t| (k.clone(), t.get("enabled").and_then(|e| e.as_bool()).unwrap_or(true))))
        .collect()
}

fn child_dirs(d: &mut Discovery, dir: &Path) -> Vec<PathBuf> {
    let read = match std::fs::read_dir(dir) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            d.unreadable.push((dir.to_path_buf(), e.to_string()));
            return Vec::new();
        }
    };
    let mut out: Vec<PathBuf> = read.flatten().map(|i| i.path()).filter(|p| p.is_dir()).collect();
    out.sort();
    out
}

fn file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// The cache keeps old version folders. Codex runs the highest one.
fn newest_version(d: &mut Discovery, plugin_dir: &Path) -> Option<(String, PathBuf)> {
    child_dirs(d, plugin_dir)
        .into_iter()
        .filter_map(|p| file_name(&p).map(|v| (v, p)))
        .max_by_key(|(v, _)| version_key(v))
}

fn version_key(v: &str) -> Vec<u64> {
    v.split('.').map(|part| part.parse::<u64>().unwrap_or(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn newest_numeric_version_wins_over_lexical_order() {
        assert!(version_key("26.1103.60933") > version_key("26.1103.32239"));
        assert!(version_key("1.10.0") > version_key("1.9.0"));
    }

    #[test]
    fn config_plugins_and_remote_installs_become_bundles() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let cache = home.join(".codex").join("plugins").join("cache");
        for (mkt, plugin, ver) in [
            ("openai-bundled", "b", "1.0"),
            ("openai-bundled", "off", "1.0"),
            ("openai-curated-remote", "r", "2.0"),
            ("openai-curated-remote", "stray", "2.0"),
        ] {
            let skill = cache.join(mkt).join(plugin).join(ver).join("skills").join("s");
            fs::create_dir_all(&skill).unwrap();
            fs::write(skill.join("SKILL.md"), "---\nname: s\n---\n").unwrap();
        }
        fs::write(cache.join("openai-curated-remote").join("r").join(REMOTE_MARKER), "{}").unwrap();
        fs::write(
            home.join(".codex").join("config.toml"),
            "[plugins.\"b@openai-bundled\"]\nenabled = true\n[plugins.\"off@openai-bundled\"]\nenabled = false\n",
        )
        .unwrap();

        let env = Env { home: home.to_path_buf(), projects: vec![], jstack_root: None };
        let d = Codex.discover(&env, &[]);

        let mut names: Vec<_> = d.bundles.iter().map(|b| (b.name.as_str(), b.enabled)).collect();
        names.sort();
        assert_eq!(names, vec![("b", true), ("off", false), ("r", true)]);
        let builtin = d.entries.iter().filter(|e| e.owner_hint == Ownership::HarnessBuiltin).count();
        assert_eq!(builtin, 2);
        assert!(d.unreadable.is_empty(), "{:?}", d.unreadable);
    }
}
