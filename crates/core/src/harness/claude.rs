//! Claude Code.
//! Skills:   ~/.claude/skills/<name>, ~/.claude/skills/synced/<id>/<name> (claude.ai, named "anthropic-skills:<name>"),
//!           <installPath>/skills/<name> per installed_plugins.json v2 (named "<plugin>:<name>"),
//!           <project>/.claude/skills/<name>.
//! Usage:    ~/.claude/projects/<slug>/*.jsonl and <slug>/<session>/subagents/*.jsonl.
//!           Model call:  assistant content item {"type":"tool_use","name":"Skill","input":{"skill":"<key>"}}
//!           Slash call:  user text containing <command-name>/<key></command-name>
//!           Every line has "timestamp", "cwd", "sessionId", "uuid".

use super::*;
use std::collections::HashMap;

pub(super) struct ClaudeCode;

/// Claude Code loads a personal skill over a project skill of the same name.
const RANK_USER: u8 = 0;
const RANK_PROJECT: u8 = 1;
const RANK_SYNCED: u8 = 2;
const RANK_PLUGIN: u8 = 3;

const SYNCED_PREFIX: &str = "anthropic-skills";
const PLUGIN_MANIFESTS: &[&str] = &[".claude-plugin/plugin.json", "plugin.json"];
const INSTALLED_PLUGINS_VERSION: i64 = 2;

impl Harness for ClaudeCode {
    fn id(&self) -> HarnessId {
        HarnessId::CLAUDE_CODE
    }
    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn discover(&self, env: &Env, projects: &[ProjectRoot]) -> Discovery {
        let mut d = Discovery::default();
        let claude = env.home.join(".claude");
        let skills = claude.join("skills");

        d.scan_skill_dir(&skills, ScopeFact::User, Ownership::Unmanaged, Naming::Plain, RANK_USER);
        discover_synced(&mut d, &skills.join("synced"));
        discover_plugins(&mut d, &claude, projects);
        for p in projects {
            let dir = p.path().join(".claude").join("skills");
            d.scan_skill_dir(&dir, ScopeFact::Project(p.clone()), Ownership::Unmanaged, Naming::Plain, RANK_PROJECT);
        }
        d
    }

    fn usage_sources(&self, env: &Env) -> Vec<UsageSource> {
        unimplemented!() // append_only: true
    }

    fn session_parser(&self, _resume: Option<&ParserState>) -> Box<dyn SessionParser> {
        // Stateless: every line carries its own cwd and session id.
        // Prefilter: line contains b"\"name\":\"Skill\"" or b"<command-name>".
        // Built-in slash commands (/clear, /model) are emitted as Slash events too. They never
        // resolve to a body. The view drops unresolved Slash keys that have no ':' so they don't show as ghosts.
        // dedup: tool_use id for Model, message uuid for Slash.
        unimplemented!()
    }

    fn bundle_command(&self, b: &BundleRef, op: BundleOp) -> Option<ExternalCommand> {
        // `claude plugin {enable|disable|uninstall|update} <name>@<marketplace>`; project scope sets cwd.
        unimplemented!()
    }
}

/// Each `synced/<id>` folder with a manifest is one claude.ai sync batch. The `synced` folder
/// itself has no SKILL.md, so only its batches are scanned.
fn discover_synced(d: &mut Discovery, synced: &Path) {
    let read = match std::fs::read_dir(synced) {
        Ok(r) => r,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => return d.unreadable.push((synced.to_path_buf(), e.to_string())),
    };
    let mut batches: Vec<PathBuf> = read.flatten().map(|i| i.path()).collect();
    batches.sort();
    for batch in batches.into_iter().filter(|b| b.join("manifest.json").is_file()) {
        d.scan_skill_dir(
            &batch,
            ScopeFact::User,
            Ownership::ClaudeAiSync,
            Naming::Prefixed(SYNCED_PREFIX.to_owned()),
            RANK_SYNCED,
        );
    }
}

fn discover_plugins(d: &mut Discovery, claude: &Path, projects: &[ProjectRoot]) {
    let path = claude.join("plugins").join("installed_plugins.json");
    let json = match fsutil::read_json(&path) {
        Ok(Some(j)) => j,
        Ok(None) => return,
        Err(e) => return d.unreadable.push((path, e)),
    };
    let version = json.get("version").and_then(|v| v.as_i64());
    if version != Some(INSTALLED_PLUGINS_VERSION) {
        let msg = format!("unsupported installed_plugins version {version:?}, expected {INSTALLED_PLUGINS_VERSION}");
        return d.unreadable.push((path, msg));
    }
    let Some(plugins) = json.get("plugins").and_then(|p| p.as_object()) else { return };

    let user_enabled = enabled_plugins(d, &[claude.join("settings.json"), claude.join("settings.local.json")]);

    let mut keys: Vec<&String> = plugins.keys().collect();
    keys.sort();
    for key in keys {
        let Some((name, marketplace)) = key.split_once('@') else {
            d.unreadable.push((path.clone(), format!("plugin key without a marketplace: {key}")));
            continue;
        };
        let installs = plugins[key].as_array().map(Vec::as_slice).unwrap_or(&[]);
        for install in installs {
            let Some(root) = install.get("installPath").and_then(|v| v.as_str()).map(PathBuf::from) else { continue };
            if !root.is_dir() {
                d.unreadable.push((root, format!("installPath of {key} is missing")));
                continue;
            }
            let (install_scope, bundle_key, enabled) = match install.get("scope").and_then(|v| v.as_str()) {
                Some("project") => {
                    let Some(project) = install.get("projectPath").and_then(|v| v.as_str()).map(PathBuf::from) else {
                        d.unreadable.push((path.clone(), format!("project-scoped {key} without projectPath")));
                        continue;
                    };
                    let project_settings = [
                        project.join(".claude").join("settings.json"),
                        project.join(".claude").join("settings.local.json"),
                    ];
                    let project_enabled = enabled_plugins(d, &project_settings);
                    let enabled = project_enabled.get(key.as_str()).or_else(|| user_enabled.get(key.as_str())).copied();
                    let bundle_key = format!("{key}#{}", norm_key(&project));
                    (InstallFact::Project(project), bundle_key, enabled)
                }
                _ => (InstallFact::User, key.clone(), user_enabled.get(key.as_str()).copied()),
            };

            d.bundles.push(BundleFact {
                key: bundle_key.clone(),
                name: name.to_owned(),
                marketplace: marketplace.to_owned(),
                version: install.get("version").and_then(|v| v.as_str()).map(str::to_owned),
                install_scope,
                enabled: enabled.unwrap_or(false),
                root: root.clone(),
            });
            d.scan_plugin_skills(
                &root,
                PLUGIN_MANIFESTS,
                ScopeFact::Bundle { key: bundle_key },
                Ownership::Unmanaged,
                Naming::Prefixed(name.to_owned()),
                RANK_PLUGIN,
            );
        }
    }
}

/// `enabledPlugins` from settings files, later files overriding earlier ones.
fn enabled_plugins(d: &mut Discovery, files: &[PathBuf]) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for file in files {
        match fsutil::read_json(file) {
            Ok(Some(json)) => {
                if let Some(map) = json.get("enabledPlugins").and_then(|e| e.as_object()) {
                    out.extend(map.iter().filter_map(|(k, v)| v.as_bool().map(|b| (k.clone(), b))));
                }
            }
            Ok(None) => {}
            Err(e) => d.unreadable.push((file.clone(), e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(home: &Path) -> Env {
        Env { home: home.to_path_buf(), projects: vec![], jstack_root: None }
    }

    #[test]
    fn plugin_bundle_enabled_state_and_members_come_from_the_json_files() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path();
        let root = home.join("cache").join("p").join("1.0");
        fs::create_dir_all(root.join("skills").join("a")).unwrap();
        fs::write(root.join("skills").join("a").join("SKILL.md"), "---\nname: a\n---\n").unwrap();
        let claude = home.join(".claude");
        fs::create_dir_all(claude.join("plugins")).unwrap();
        let installed = format!(
            r#"{{"version":2,"plugins":{{"p@m":[{{"scope":"user","installPath":{},"version":"1.0"}}],"q@m":[{{"scope":"user","installPath":{}}}]}}}}"#,
            serde_json::to_string(&root).unwrap(),
            serde_json::to_string(&root).unwrap()
        );
        fs::write(claude.join("plugins").join("installed_plugins.json"), installed).unwrap();
        fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"p@m":true,"q@m":false}}"#).unwrap();

        let d = ClaudeCode.discover(&env(home), &[]);

        assert_eq!(d.bundles.len(), 2);
        let p = d.bundles.iter().find(|b| b.name == "p").unwrap();
        let q = d.bundles.iter().find(|b| b.name == "q").unwrap();
        assert!(p.enabled && !q.enabled);
        assert_eq!(p.version.as_deref(), Some("1.0"));
        assert_eq!(d.entries.len(), 2);
        assert!(d.entries.iter().all(|e| matches!(&e.scope, ScopeFact::Bundle { .. })));
        assert!(d.unreadable.is_empty(), "{:?}", d.unreadable);
    }

    #[test]
    fn unsupported_installed_plugins_version_is_reported() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(".claude").join("plugins");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("installed_plugins.json"), r#"{"version":9,"plugins":{}}"#).unwrap();
        let d = ClaudeCode.discover(&env(t.path()), &[]);
        assert!(d.bundles.is_empty());
        assert_eq!(d.unreadable.len(), 1);
    }
}
