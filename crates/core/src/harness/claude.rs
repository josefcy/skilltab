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
        discover_plugins(&mut d, &claude);
        for p in projects {
            let dir = p.path().join(".claude").join("skills");
            d.scan_skill_dir(&dir, ScopeFact::Project(p.clone()), Ownership::Unmanaged, Naming::Plain, RANK_PROJECT);
        }
        d
    }

    fn usage_sources(&self, env: &Env) -> Vec<UsageSource> {
        let mut out = Vec::new();
        let Ok(slugs) = std::fs::read_dir(env.home.join(".claude").join("projects")) else { return out };
        for slug in slugs.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
            push_jsonl(&mut out, &slug);
            let Ok(sessions) = std::fs::read_dir(&slug) else { continue };
            for session in sessions.flatten().map(|e| e.path()).filter(|p| p.is_dir()) {
                push_jsonl(&mut out, &session.join("subagents"));
            }
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// Stateless: every line carries its own cwd and session id, so `resume` is unused.
    fn session_parser(&self, _resume: Option<&ParserState>) -> Box<dyn SessionParser> {
        Box::new(ClaudeParser::new())
    }

    /// Only user-scope installs: which `--scope` flag a project install needs is unverified, and
    /// without it the CLI would flip the user-wide setting instead.
    fn bundle_command(&self, b: &BundleRef, op: BundleOp) -> Option<ExternalCommand> {
        if *b.install_scope != InstallScope::User {
            return None;
        }
        let verb = match op {
            BundleOp::Enable => "enable",
            BundleOp::Disable => "disable",
        };
        Some(ExternalCommand {
            program: "claude".to_owned(),
            args: vec!["plugin".to_owned(), verb.to_owned(), format!("{}@{}", b.name, b.marketplace)],
            cwd: None,
            env: Vec::new(),
        })
    }

    fn setup(&self) -> HarnessSetup {
        HarnessSetup {
            config_home: ".claude",
            carry: &[Carry::File("CLAUDE.md"), Carry::Dir { dir: "rules", ext: "md" }],
            skills_dir: Some(".claude/skills"),
            skills_cli_agent: Some("claude-code"),
            tools: &["claude"],
            sign_in: Some("Run claude and follow the login prompt."),
            install_hint: Some("Install Claude Code from https://claude.com/claude-code."),
        }
    }

    /// A marketplace is present when `known_marketplaces.json` names it, whatever its source type, and a plugin
    /// when `installed_plugins.json` does.
    fn plugin_ops(&self, want: &SetupManifest, have: &PluginRecord) -> Vec<PluginOp> {
        let command = |args: &[&str], target: &str| ExternalCommand {
            program: "claude".to_owned(),
            args: args.iter().map(|a| (*a).to_owned()).chain([target.to_owned()]).collect(),
            cwd: None,
            env: Vec::new(),
        };
        let marketplaces = want.marketplaces.iter().map(|(name, source)| {
            let known = have.marketplaces.contains_key(name) || have.uncarried.iter().any(|(n, _)| n == name.as_str());
            let state = if known {
                PluginState::Present
            } else {
                let from = match source {
                    MarketplaceSource::Github { repo } => repo.as_str(),
                    MarketplaceSource::Git { url } => url.as_str(),
                };
                PluginState::Missing(command(&["plugin", "marketplace", "add"], from))
            };
            PluginOp { label: format!("marketplace {}", name.as_str()), state }
        });
        let plugins = want.plugins.iter().map(|plugin| {
            let state = if have.installed.contains(plugin.as_str()) {
                PluginState::Present
            } else {
                PluginState::Missing(command(&["plugin", "install"], plugin.as_str()))
            };
            PluginOp { label: format!("plugin {}", plugin.as_str()), state }
        });
        marketplaces.chain(plugins).collect()
    }

    /// Reads the same three files `backup.ps1` reads, and the installed set for Restore. Only user
    /// `settings.json` counts for enabled plugins, and only a value of true.
    fn read_plugins(&self, env: &Env) -> Result<PluginRecord, String> {
        let plugins = env.home.join(".claude").join("plugins");
        let read = |path: &Path| fsutil::read_json(path).map_err(|e| format!("Could not read {}: {e}.", path.display()));
        let mut record = PluginRecord::default();

        let known = plugins.join("known_marketplaces.json");
        if let Some(json) = read(&known)? {
            let map = json.as_object().ok_or_else(|| format!("{} is not a JSON object.", known.display()))?;
            for (name, entry) in map {
                let field = |k: &str| entry.get("source").and_then(|s| s.get(k)).and_then(|v| v.as_str()).filter(|v| !v.is_empty());
                let kind = field("source");
                let arg = |what: &str, v: &str| Arg::parse(v).map_err(|e| format!("Marketplace \"{name}\" in {}: {what} {e}.", known.display()));
                let source = match (kind, field("repo"), field("url")) {
                    (Some(k), Some(repo), _) if k.eq_ignore_ascii_case("github") => MarketplaceSource::Github { repo: arg("repo", repo)? },
                    (Some(k), _, Some(url)) if k.eq_ignore_ascii_case("git") => MarketplaceSource::Git { url: arg("url", url)? },
                    _ => {
                        record.uncarried.push((name.clone(), kind.unwrap_or("unknown").to_owned()));
                        continue;
                    }
                };
                let name = Arg::parse(name).map_err(|e| format!("{}: the marketplace name {e}.", known.display()))?;
                record.marketplaces.insert(name, source);
            }
        }

        let settings = env.home.join(".claude").join("settings.json");
        if let Some(json) = read(&settings)?
            && let Some(map) = json.get("enabledPlugins").and_then(|e| e.as_object())
        {
            for key in map.iter().filter(|(_, v)| v.as_bool() == Some(true)).map(|(k, _)| k) {
                record.enabled.insert(Arg::parse(key).map_err(|e| format!("{}: the plugin {e}.", settings.display()))?);
            }
        }

        if let Some(json) = read(&plugins.join("installed_plugins.json"))?
            && let Some(map) = json.get("plugins").and_then(|p| p.as_object())
        {
            record.installed = map.keys().cloned().collect();
        }
        Ok(record)
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

fn discover_plugins(d: &mut Discovery, claude: &Path) {
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

/// Counts two line shapes and nothing else.
///   Model: an `assistant` line whose message content holds `{"type":"tool_use","name":"Skill","input":{"skill":K}}`.
///   Slash: a `user` line whose message content is text that starts with `<command-message>` or
///          `<command-name>` and names `<command-name>/K</command-name>`.
/// Every other line that mentions these markers is skipped on purpose: the tool catalogs in
/// `api-request-*` and `attachment` lines list `"name":"Skill"` and `<command-name>` in their
/// descriptions, and `api-request-blob` lines replay earlier messages. Built-in commands such as
/// `/compact` are kept, since only the snapshot can tell whether a key is a skill.
struct ClaudeParser {
    skill_call: memchr::memmem::Finder<'static>,
    command: memchr::memmem::Finder<'static>,
    assistant: memchr::memmem::Finder<'static>,
    user: memchr::memmem::Finder<'static>,
}

impl ClaudeParser {
    fn new() -> ClaudeParser {
        use memchr::memmem::Finder;
        ClaudeParser {
            skill_call: Finder::new(br#""name":"Skill""#),
            command: Finder::new(b"<command-name>"),
            assistant: Finder::new(br#""type":"assistant""#),
            user: Finder::new(br#""type":"user""#),
        }
    }
}

#[derive(serde::Deserialize)]
struct Record {
    #[serde(rename = "type")]
    ty: String,
    uuid: Option<String>,
    timestamp: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<Message>,
}

#[derive(serde::Deserialize)]
struct Message {
    content: Option<Content>,
}

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum Content {
    Text(String),
    Items(Vec<Item>),
}

#[derive(serde::Deserialize)]
struct Item {
    #[serde(rename = "type")]
    ty: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
    text: Option<String>,
}

impl SessionParser for ClaudeParser {
    fn feed(&mut self, line: &[u8], out: &mut Vec<RawEvent>) {
        let model = self.skill_call.find(line).is_some() && self.assistant.find(line).is_some();
        let slash = self.command.find(line).is_some() && self.user.find(line).is_some();
        if !model && !slash {
            return;
        }
        let Ok(rec) = serde_json::from_slice::<Record>(line) else { return };
        let Some(at) = rec.timestamp.as_deref().and_then(parse_ts) else { return };
        let Some(content) = rec.message.and_then(|m| m.content) else { return };
        let session = rec.session_id.unwrap_or_default();
        let cwd = rec.cwd.map(PathBuf::from);
        let mut emit = |key: String, kind: InvocationKind, dedup: String| {
            out.push(RawEvent { key: InvocationKey::Name(key), kind, at, session: session.clone(), cwd: cwd.clone(), dedup });
        };

        match (rec.ty.as_str(), content) {
            ("assistant", Content::Items(items)) if model => {
                for item in items {
                    if item.ty.as_deref() != Some("tool_use") || item.name.as_deref() != Some("Skill") {
                        continue;
                    }
                    let key = item.input.as_ref().and_then(|i| i.get("skill")).and_then(|s| s.as_str());
                    if let (Some(key), Some(id)) = (key.map(str::trim).filter(|k| !k.is_empty()), item.id) {
                        emit(key.to_owned(), InvocationKind::Model, format!("tool:{id}"));
                    }
                }
            }
            ("user", content) if slash => {
                let Some(uuid) = rec.uuid else { return };
                let texts: Vec<String> = match content {
                    Content::Text(t) => vec![t],
                    Content::Items(items) => items.into_iter().filter_map(|i| i.text).collect(),
                };
                if let Some(name) = texts.iter().find_map(|t| slash_command(t)) {
                    emit(name.to_owned(), InvocationKind::Slash, format!("msg:{uuid}"));
                }
            }
            _ => {}
        }
    }

    fn checkpoint(&self) -> ParserState {
        ParserState(String::new())
    }
}

/// The command in `<command-message>x</command-message><command-name>/x</command-name>...`, without its slash.
/// A message that only quotes the tag further along is not a command.
fn slash_command(text: &str) -> Option<&str> {
    let text = text.trim_start();
    if !text.starts_with("<command-message>") && !text.starts_with("<command-name>") {
        return None;
    }
    const OPEN: &str = "<command-name>";
    let start = text.find(OPEN)? + OPEN.len();
    let end = start + text[start..].find("</command-name>")?;
    let name = text[start..end].trim().trim_start_matches('/');
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn env(home: &Path) -> Env {
        Env { home: home.to_path_buf(), projects: vec![] }
    }

    fn feed(line: serde_json::Value) -> Vec<RawEvent> {
        let mut out = Vec::new();
        ClaudeParser::new().feed(line.to_string().as_bytes(), &mut out);
        out
    }

    fn base(ty: &str, message: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"type": ty, "uuid": "u1", "sessionId": "s1", "cwd": "D:\\p",
            "timestamp": "2026-11-01T10:00:00.000Z", "message": message})
    }

    #[test]
    fn skill_tool_use_is_a_model_event_keyed_by_tool_use_id() {
        let items = serde_json::json!([
            {"type": "text", "text": "hi"},
            {"type": "tool_use", "id": "toolu_1", "name": "Skill", "input": {"skill": "pstack:how", "args": "x"}},
            {"type": "tool_use", "id": "toolu_2", "name": "Bash", "input": {"command": "ls"}},
        ]);
        let events = feed(base("assistant", serde_json::json!({"role": "assistant", "content": items})));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, InvocationKey::Name("pstack:how".into()));
        assert_eq!(events[0].kind, InvocationKind::Model);
        assert_eq!(events[0].dedup, "tool:toolu_1");
        assert_eq!(events[0].session, "s1");
        assert_eq!(events[0].cwd, Some(PathBuf::from("D:\\p")));
    }

    #[test]
    fn slash_command_is_a_slash_event_keyed_by_message_uuid() {
        let text = "<command-message>implement</command-message>\n<command-name>/mattpocock-skills:implement</command-name>\n<command-args>x</command-args>";
        let events = feed(base("user", serde_json::json!({"role": "user", "content": text})));
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, InvocationKey::Name("mattpocock-skills:implement".into()));
        assert_eq!(events[0].kind, InvocationKind::Slash);
        assert_eq!(events[0].dedup, "msg:u1");

        let builtin = "<command-name>/compact</command-name>\n<command-message>compact</command-message>";
        let events = feed(base("user", serde_json::json!({"role": "user", "content": builtin})));
        assert_eq!(events[0].key, InvocationKey::Name("compact".into()));
    }

    #[test]
    fn tool_catalogs_replayed_requests_and_quotations_are_not_usage() {
        let desc = "Run <command-name>/x</command-name> via the \"name\":\"Skill\" tool";
        let catalog = serde_json::json!({"type": "attachment", "attachment": {"tools": [{"name": "Skill", "description": desc}]},
            "timestamp": "2026-11-01T10:00:00.000Z", "uuid": "u2"});
        assert!(feed(catalog).is_empty());

        let shape = serde_json::json!({"type": "api-request-shape", "timestamp": "2026-11-01T10:00:00.000Z",
            "shape": {"tools": [{"name": "Skill", "description": desc}]}});
        assert!(feed(shape).is_empty());

        let replay = serde_json::json!({"type": "api-request-blob", "timestamp": "2026-11-01T10:00:00.000Z", "message": {"content": [
            {"type": "tool_use", "id": "toolu_9", "name": "Skill", "input": {"skill": "x"}},
            {"type": "text", "text": "<command-name>/y</command-name>"}]}});
        assert!(feed(replay).is_empty());

        let quoted = base("user", serde_json::json!({"role": "user", "content": "why does <command-name>/y</command-name> appear?"}));
        assert!(feed(quoted).is_empty());

        let tool_result = base("user", serde_json::json!({"role": "user", "content": [
            {"type": "tool_result", "content": "<command-name>/y</command-name>"}]}));
        assert!(feed(tool_result).is_empty());

        let assistant_text = base("assistant", serde_json::json!({"content": [
            {"type": "text", "text": "<command-name>/y</command-name> and \"name\":\"Skill\""}]}));
        assert!(feed(assistant_text).is_empty());
    }

    #[test]
    fn sources_cover_sessions_and_subagents_but_not_other_files() {
        let t = tempfile::tempdir().unwrap();
        let slug = t.path().join(".claude").join("projects").join("D--p");
        let sub = slug.join("sess").join("subagents");
        fs::create_dir_all(&sub).unwrap();
        for f in [slug.join("a.jsonl"), sub.join("agent-1.jsonl"), sub.join("agent-1.meta.json"), slug.join("notes.txt")] {
            fs::write(f, "").unwrap();
        }
        let names: Vec<String> = ClaudeCode
            .usage_sources(&env(t.path()))
            .iter()
            .map(|s| s.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["a.jsonl", "agent-1.jsonl"]);
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

    fn names<T: AsRef<str>>(items: impl IntoIterator<Item = T>) -> Vec<String> {
        items.into_iter().map(|i| i.as_ref().to_owned()).collect()
    }

    #[test]
    fn plugin_records_split_carried_marketplaces_from_ones_the_format_cannot_rebuild() {
        let home = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/setup/home");
        let record = ClaudeCode.read_plugins(&env(&home)).unwrap();

        assert_eq!(names(record.marketplaces.keys().map(Arg::as_str)), ["Beta-market", "alpha-market", "zeta-market"]);
        assert_eq!(record.uncarried, [("local-market".to_owned(), "directory".to_owned())]);
        assert_eq!(names(record.enabled.iter().map(Arg::as_str)), ["plug-a@zeta-market", "plug-b@alpha-market"], "false is not enabled");
        assert!(record.installed.is_empty());
    }

    #[test]
    fn only_user_settings_count_and_only_a_value_of_true() {
        let t = tempfile::tempdir().unwrap();
        let claude = t.path().join(".claude");
        fs::create_dir_all(claude.join("plugins")).unwrap();
        fs::write(claude.join("settings.json"), r#"{"enabledPlugins":{"a@m":true,"b@m":"true","c@m":1}}"#).unwrap();
        fs::write(claude.join("settings.local.json"), r#"{"enabledPlugins":{"d@m":true}}"#).unwrap();
        fs::write(claude.join("plugins").join("installed_plugins.json"), r#"{"version":2,"plugins":{"a@m":[],"x@m":[]}}"#).unwrap();

        let record = ClaudeCode.read_plugins(&env(t.path())).unwrap();
        assert_eq!(names(record.enabled.iter().map(Arg::as_str)), ["a@m"]);
        assert_eq!(names(&record.installed), ["a@m", "x@m"]);
    }

    #[test]
    fn a_missing_home_has_no_plugins_and_an_unreadable_record_is_an_error_that_names_the_file() {
        let t = tempfile::tempdir().unwrap();
        let record = ClaudeCode.read_plugins(&env(t.path())).unwrap();
        assert!(record.marketplaces.is_empty() && record.enabled.is_empty() && record.uncarried.is_empty());

        let claude = t.path().join(".claude");
        fs::create_dir_all(claude.join("plugins")).unwrap();
        for (file, text) in [("plugins/known_marketplaces.json", "{oops"), ("settings.json", "[1,"), ("plugins/installed_plugins.json", "nope")] {
            fs::write(claude.join(file), text).unwrap();
            let e = ClaudeCode.read_plugins(&env(t.path())).unwrap_err();
            assert!(e.contains(file.rsplit('/').next().unwrap()), "{e}");
            fs::remove_file(claude.join(file)).unwrap();
        }
    }

    #[test]
    fn a_marketplace_with_a_flag_for_a_repo_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join(".claude").join("plugins");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("known_marketplaces.json"), r#"{"m":{"source":{"source":"github","repo":"--evil"}}}"#).unwrap();
        assert!(ClaudeCode.read_plugins(&env(t.path())).unwrap_err().contains("\"m\""));
    }

    fn wanted() -> SetupManifest {
        SetupManifest::parse(
            r#"{"marketplaces":[
                {"name":"alpha","source":{"source":"github","repo":"acme/alpha"}},
                {"name":"beta","source":{"source":"git","url":"https://example.com/acme/beta.git"}}],
              "claudePlugins":["plug@alpha","other@beta"]}"#,
        )
        .unwrap()
    }

    fn describe(ops: &[PluginOp]) -> Vec<String> {
        ops.iter()
            .map(|op| match &op.state {
                PluginState::Present => format!("{}: present", op.label),
                PluginState::Missing(c) => format!("{}: {}", op.label, c.display()),
            })
            .collect()
    }

    #[test]
    fn plugin_ops_name_the_claude_command_for_everything_missing_marketplaces_first() {
        assert_eq!(
            describe(&ClaudeCode.plugin_ops(&wanted(), &PluginRecord::default())),
            [
                "marketplace alpha: claude plugin marketplace add acme/alpha",
                "marketplace beta: claude plugin marketplace add https://example.com/acme/beta.git",
                "plugin other@beta: claude plugin install other@beta",
                "plugin plug@alpha: claude plugin install plug@alpha",
            ]
        );
    }

    #[test]
    fn plugin_ops_skip_what_the_records_already_hold() {
        let mut have = PluginRecord::default();
        have.marketplaces.insert(Arg::parse("alpha").unwrap(), MarketplaceSource::Github { repo: Arg::parse("acme/alpha").unwrap() });
        have.uncarried.push(("beta".to_owned(), "directory".to_owned()));
        have.installed.insert("plug@alpha".to_owned());
        assert_eq!(
            describe(&ClaudeCode.plugin_ops(&wanted(), &have)),
            [
                "marketplace alpha: present",
                "marketplace beta: present",
                "plugin other@beta: claude plugin install other@beta",
                "plugin plug@alpha: present",
            ]
        );
    }
}
