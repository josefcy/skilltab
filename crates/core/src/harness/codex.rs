//! Codex.
//! Skills:   ~/.agents/skills/<name> (Codex loads it directly),
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
/// Where Codex looks for the plugin name of a skill folder. A skill that sits in a Claude Code
/// plugin cache (the pstack junctions) is namespaced by its `.claude-plugin` manifest.
const NAMESPACE_MANIFESTS: &[&str] = &[".codex-plugin/plugin.json", ".claude-plugin/plugin.json"];
const PLUGIN_ROOT_DEPTH: usize = 5;
/// sessions/YYYY/MM/DD/rollout-*.jsonl
const ROLLOUT_DEPTH: u8 = 4;

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
        namespace_plugin_skills(&mut d.entries);
        discover_plugins(&mut d, &codex);
        d
    }

    fn usage_sources(&self, env: &Env) -> Vec<UsageSource> {
        let codex = env.home.join(".codex");
        let mut out = Vec::new();
        collect_rollouts(&mut out, &codex.join("sessions"), 0);
        // Archived rollouts are moved out of `sessions`, so they never repeat a live file's events.
        push_jsonl(&mut out, &codex.join("archived_sessions"));
        out.retain(|s| s.path.file_name().is_some_and(|n| n.to_string_lossy().starts_with("rollout-")));
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    /// The `session_meta` line at the top of a rollout gives the session id and cwd that later
    /// lines rely on, so they travel in the parser state across resumed reads.
    fn session_parser(&self, resume: Option<&ParserState>) -> Box<dyn SessionParser> {
        Box::new(CodexParser::new(resume))
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

/// Codex shows a skill as `<plugin>:<name>` whenever the skill's real folder sits inside a plugin,
/// even when it is reached through `~/.agents/skills` or `~/.codex/skills`. Its available-skills
/// list names the pstack junctions in `~/.agents/skills` that way. The scanner can only give
/// `Plain` keys to those mounts, so the plugin name is found here, by following the link.
fn namespace_plugin_skills(entries: &mut [EntryFact]) {
    for entry in entries {
        if let Some(plugin) = owning_plugin(&entry.entry) {
            entry.naming = Naming::Prefixed(plugin);
        }
    }
}

fn owning_plugin(entry: &Path) -> Option<String> {
    let real = fsutil::strip_verbatim(std::fs::canonicalize(entry).ok()?);
    real.ancestors().skip(1).take(PLUGIN_ROOT_DEPTH).find_map(|dir| {
        NAMESPACE_MANIFESTS.iter().find_map(|m| {
            let manifest = fsutil::read_json(&dir.join(m)).ok()??;
            manifest.get("name")?.as_str().filter(|n| !n.is_empty()).map(str::to_owned)
        })
    })
}

fn collect_rollouts(out: &mut Vec<UsageSource>, dir: &Path, depth: u8) {
    if depth > ROLLOUT_DEPTH {
        return;
    }
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for entry in read.flatten() {
        match entry.file_type() {
            Ok(t) if t.is_dir() => subdirs.push(entry.path()),
            _ => {}
        }
    }
    push_jsonl(out, dir);
    for sub in subdirs {
        collect_rollouts(out, &sub, depth + 1);
    }
}

/// Counts two line shapes and nothing else.
///   Mention:  a `user` message item `<skill><name>N</name><path>P</path>...</skill>`. Codex writes it
///             when the user picked a skill with `$N`, so it is the harness confirming the mention
///             resolved. A bare `$N` in prose is not counted, since it is often a shell variable.
///   FileRead: a `custom_tool_call` or `function_call` whose code reads an absolute `...\SKILL.md`
///             through a plain read command (Get-Content, cat, type and similar). A path that is only
///             listed, patched, or passed to another program is not counted.
/// The "## Skills" listing in developer messages is availability, not use, and is ignored.
struct CodexParser {
    session: String,
    cwd: Option<String>,
    session_meta: memchr::memmem::Finder<'static>,
    user_role: memchr::memmem::Finder<'static>,
    skill_block: memchr::memmem::Finder<'static>,
    skill_md: memchr::memmem::Finder<'static>,
    custom_call: memchr::memmem::Finder<'static>,
    function_call: memchr::memmem::Finder<'static>,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct CodexState {
    session: String,
    cwd: Option<String>,
}

impl CodexParser {
    fn new(resume: Option<&ParserState>) -> CodexParser {
        use memchr::memmem::Finder;
        let state: CodexState = resume.and_then(|s| serde_json::from_str(&s.0).ok()).unwrap_or_default();
        CodexParser {
            session: state.session,
            cwd: state.cwd,
            session_meta: Finder::new(br#""type":"session_meta""#),
            user_role: Finder::new(br#""role":"user""#),
            skill_block: Finder::new(b"<skill>"),
            skill_md: Finder::new(b"SKILL.md"),
            custom_call: Finder::new(br#""type":"custom_tool_call""#),
            function_call: Finder::new(br#""type":"function_call""#),
        }
    }
}

impl SessionParser for CodexParser {
    fn feed(&mut self, line: &[u8], out: &mut Vec<RawEvent>) {
        let meta = self.session_meta.find(line).is_some();
        let mention = self.skill_block.find(line).is_some() && self.user_role.find(line).is_some();
        let read = self.skill_md.find(line).is_some()
            && (self.custom_call.find(line).is_some() || self.function_call.find(line).is_some());
        if !meta && !mention && !read {
            return;
        }
        let Ok(rec) = serde_json::from_slice::<serde_json::Value>(line) else { return };
        let payload = &rec["payload"];
        match rec["type"].as_str() {
            Some("session_meta") => {
                if let Some(id) = payload["id"].as_str().or_else(|| payload["session_id"].as_str()) {
                    self.session = id.to_owned();
                }
                self.cwd = payload["cwd"].as_str().map(str::to_owned);
            }
            Some("response_item") => {
                let Some(at) = rec["timestamp"].as_str().and_then(parse_ts) else { return };
                let event = |key: InvocationKey, kind: InvocationKind, dedup: String| RawEvent {
                    key,
                    kind,
                    at,
                    session: self.session.clone(),
                    cwd: self.cwd.as_deref().map(PathBuf::from),
                    dedup,
                };
                match payload["type"].as_str() {
                    Some("message") if mention && payload["role"] == "user" => {
                        let message_id = payload["id"]
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| format!("{}:{}", self.session, rec["ordinal"]));
                        let blocks = payload["content"].as_array().into_iter().flatten();
                        for name in blocks.filter_map(|i| i["text"].as_str()).filter_map(skill_block_name) {
                            out.push(event(
                                InvocationKey::Name(name.to_owned()),
                                InvocationKind::Mention,
                                format!("mention:{message_id}:{name}"),
                            ));
                        }
                    }
                    Some("custom_tool_call" | "function_call") if read => {
                        let Some(call_id) = payload["call_id"].as_str() else { return };
                        let code = payload["input"].as_str().or_else(|| payload["arguments"].as_str()).unwrap_or("");
                        for path in read_skill_paths(code) {
                            let key = canonical_skill_file(&path);
                            let dedup = format!("read:{call_id}:{}", crate::model::norm_key(&key));
                            out.push(event(InvocationKey::SkillFile(key), InvocationKind::FileRead, dedup));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn checkpoint(&self) -> ParserState {
        let state = CodexState { session: self.session.clone(), cwd: self.cwd.clone() };
        ParserState(serde_json::to_string(&state).unwrap_or_default())
    }
}

/// The skill name in `<skill>\n<name>N</name>\n<path>...</path>...`.
fn skill_block_name(text: &str) -> Option<&str> {
    let body = text.trim_start().strip_prefix("<skill>")?;
    let start = body.find("<name>")? + "<name>".len();
    let end = start + body[start..].find("</name>")?;
    let name = body[start..end].trim();
    (!name.is_empty()).then_some(name)
}

const READ_COMMANDS: &[&str] =
    &["get-content", "gc", "cat", "type", "more", "less", "bat", "readalltext", "readalllines", "readfilesync"];

/// Absolute `SKILL.md` paths that `code` hands to a read command. `code` is the JS (or JSON) text of
/// a tool call, so Windows backslashes arrive doubled. A transcript names the paths of the machine
/// that wrote it, so a drive path and a leading `/` both count on every OS.
fn read_skill_paths(code: &str) -> Vec<String> {
    const FILE: &str = "SKILL.md";
    let text = code.replace("\\\\", "\\");
    let mut out: Vec<String> = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find(FILE) {
        let end = from + i + FILE.len();
        from = end;
        let continues = text[end..].chars().next().is_some_and(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_'));
        let begin = text[..end].rfind(is_path_delimiter).map_or(0, |d| d + 1);
        let path = &text[begin..end];
        let b = path.as_bytes();
        let drive = b.len() > 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/');
        let absolute = drive || b[0] == b'/';
        if !continues && absolute && read_command_before(&text[..begin]) && !out.iter().any(|p| p == path) {
            out.push(path.to_owned());
        }
    }
    out
}

fn is_path_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ';' | ',' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '<' | '>' | '=')
}

/// Is the nearest word before the path a read command, once flags such as `-Raw` and `-TotalCount 130` are skipped?
fn read_command_before(prefix: &str) -> bool {
    for token in prefix.rsplit(char::is_whitespace).filter(|t| !t.is_empty()).take(6) {
        let word = token.rsplit(|c: char| !(c.is_alphanumeric() || c == '-' || c == '_')).find(|w| !w.is_empty());
        let Some(word) = word else { continue };
        if word.starts_with('-') || word.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        return READ_COMMANDS.contains(&word.to_ascii_lowercase().as_str());
    }
    false
}

/// A read through a junction and a read of the real folder must give one key, the same one the
/// scanner gives that folder. A folder that is gone keeps the spelling it was read under.
fn canonical_skill_file(path: &str) -> PathBuf {
    #[cfg(windows)]
    let path = PathBuf::from(path.replace('/', "\\"));
    #[cfg(not(windows))]
    let path = PathBuf::from(path);
    match path.parent().map(std::fs::canonicalize) {
        Some(Ok(dir)) => fsutil::strip_verbatim(dir).join("SKILL.md"),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::link;
    use std::fs;

    fn feed_all(parser: &mut CodexParser, lines: &[serde_json::Value]) -> Vec<RawEvent> {
        let mut out = Vec::new();
        for l in lines {
            parser.feed(l.to_string().as_bytes(), &mut out);
        }
        out
    }

    fn meta() -> serde_json::Value {
        serde_json::json!({"timestamp": "2026-11-01T10:00:00.000Z", "type": "session_meta",
            "payload": {"id": "sess-1", "cwd": "D:\\proj"}})
    }

    fn user_message(id: &str, texts: &[&str]) -> serde_json::Value {
        let content: Vec<_> = texts.iter().map(|t| serde_json::json!({"type": "input_text", "text": t})).collect();
        serde_json::json!({"timestamp": "2026-11-01T10:00:01.000Z", "type": "response_item",
            "payload": {"type": "message", "id": id, "role": "user", "content": content}})
    }

    fn exec_call(call_id: &str, code: &str) -> serde_json::Value {
        serde_json::json!({"timestamp": "2026-11-01T10:00:02.000Z", "type": "response_item",
            "payload": {"type": "custom_tool_call", "call_id": call_id, "name": "exec", "input": code}})
    }

    #[test]
    fn skill_blocks_are_mentions_and_bare_dollar_text_is_not() {
        let block = "<skill>\n<name>pstack:architect</name>\n<path>C:\\Users\\u\\.agents\\skills\\architect\\SKILL.md</path>\nbody\n</skill>";
        let developer = serde_json::json!({"timestamp": "2026-11-01T10:00:00.500Z", "type": "response_item",
            "payload": {"type": "message", "role": "developer", "content": [
                {"type": "input_text", "text": "<skill><name>zzz</name></skill> - architect: (file: r1/architect/SKILL.md)"}]}});
        let lines = [meta(), developer, user_message("m1", &["use $architect and $HOME"]), user_message("m2", &[block])];
        let events = feed_all(&mut CodexParser::new(None), &lines);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].key, InvocationKey::Name("pstack:architect".into()));
        assert_eq!(events[0].kind, InvocationKind::Mention);
        assert_eq!(events[0].dedup, "mention:m2:pstack:architect");
        assert_eq!(events[0].session, "sess-1");
        assert_eq!(events[0].cwd, Some(PathBuf::from("D:\\proj")));
    }

    #[test]
    fn only_read_commands_on_absolute_skill_paths_are_file_reads() {
        let counted = [
            r#"const r = await tools.exec_command({cmd:"Get-Content C:/Users/u/.agents/skills/a/SKILL.md; Get-Content C:/Users/u/.codex/skills/b/SKILL.md"});"#,
            r#"text(await tools.exec_command({"cmd":"Get-Content -Raw 'C:\\Users\\u\\.codex\\skills\\c\\SKILL.md'"}))"#,
            r#"tools.exec_command({cmd:"Get-Content -LiteralPath 'C:/x/d/SKILL.md' -TotalCount 40"})"#,
            r#"tools.exec_command({cmd:"cat C:/x/e/SKILL.md"})"#,
            r#"tools.exec_command({cmd:"cat /home/u/.codex/skills/f/SKILL.md"})"#,
        ];
        let ignored = [
            r#"tools.exec_command({cmd:"Get-ChildItem C:/Users/u/.agents/skills/a/SKILL.md"})"#,
            r#"tools.exec_command({cmd:"Get-Content AGENTS.md; Get-Content .agents/skills/a/SKILL.md"})"#,
            r#"const patch = "*** Begin Patch\n*** Update File: C:\\Users\\u\\.codex\\skills\\a\\SKILL.md\n";"#,
            r#"tools.exec_command({cmd:"Get-Content C:/x/a/SKILL.md.bak"})"#,
            r#"tools.exec_command({cmd:"node run.js C:/x/a/SKILL.md"})"#,
        ];
        let mut p = CodexParser::new(None);
        let mut lines = vec![meta()];
        lines.extend(counted.iter().enumerate().map(|(i, c)| exec_call(&format!("c{i}"), c)));
        lines.extend(ignored.iter().enumerate().map(|(i, c)| exec_call(&format!("i{i}"), c)));
        let events = feed_all(&mut p, &lines);

        let mut keys: Vec<String> = events
            .iter()
            .map(|e| match &e.key {
                InvocationKey::SkillFile(p) => p.to_string_lossy().replace('\\', "/"),
                other => panic!("{other:?}"),
            })
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "/home/u/.codex/skills/f/SKILL.md",
                "C:/Users/u/.agents/skills/a/SKILL.md",
                "C:/Users/u/.codex/skills/b/SKILL.md",
                "C:/Users/u/.codex/skills/c/SKILL.md",
                "C:/x/d/SKILL.md",
                "C:/x/e/SKILL.md",
            ]
        );
        assert!(events.iter().all(|e| e.kind == InvocationKind::FileRead));
    }

    #[test]
    fn parser_state_carries_session_and_cwd_across_a_resumed_read() {
        let mut first = CodexParser::new(None);
        feed_all(&mut first, &[meta()]);
        let state = first.checkpoint();

        let mut resumed = CodexParser::new(Some(&state));
        let block = "<skill><name>x</name><path>P</path></skill>";
        let events = feed_all(&mut resumed, &[user_message("m1", &[block])]);
        assert_eq!(events[0].session, "sess-1");
        assert_eq!(events[0].cwd, Some(PathBuf::from("D:\\proj")));
    }

    #[test]
    fn a_read_through_a_junction_keys_on_the_real_folder() {
        let t = tempfile::tempdir().unwrap();
        let real = fsutil::strip_verbatim(fs::canonicalize(t.path()).unwrap()).join("store").join("a");
        fs::create_dir_all(&real).unwrap();
        fs::write(real.join("SKILL.md"), "---\nname: a\n---\n").unwrap();
        let entry = t.path().join("agents").join("a");
        link(&entry, &real);

        let code = format!("Get-Content {}", entry.join("SKILL.md").to_string_lossy().replace('\\', "/"));
        let events = feed_all(&mut CodexParser::new(None), &[meta(), exec_call("c1", &code)]);
        assert_eq!(events[0].key, InvocationKey::SkillFile(real.join("SKILL.md")));
    }

    #[test]
    fn a_junction_into_a_plugin_cache_is_named_like_codex_names_it() {
        let t = tempfile::tempdir().unwrap();
        let home = fsutil::strip_verbatim(fs::canonicalize(t.path()).unwrap());
        let version = home.join("cache").join("pstack").join("1.0");
        let skill = version.join("skills").join("architect");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "---\nname: architect\n---\n").unwrap();
        fs::create_dir_all(version.join(".claude-plugin")).unwrap();
        fs::write(version.join(".claude-plugin").join("plugin.json"), r#"{"name":"pstack"}"#).unwrap();
        let plain = home.join("repo").join("tdd");
        fs::create_dir_all(&plain).unwrap();
        fs::write(plain.join("SKILL.md"), "---\nname: tdd\n---\n").unwrap();

        let agents = home.join(".agents").join("skills");
        link(&agents.join("architect"), &skill);
        link(&agents.join("tdd"), &plain);

        let env = Env { home: home.clone(), projects: vec![] };
        let snap = crate::scan::scan(&env, &[&Codex], &[], &crate::provenance::Provenance::load(&env), Vec::new(), 1);

        let id = snap.resolve(HarnessId::CODEX, &InvocationKey::Name("pstack:architect".into()));
        assert!(id.is_some(), "Codex lists the pstack junction as pstack:architect");
        assert!(snap.resolve(HarnessId::CODEX, &InvocationKey::Name("architect".into())).is_none());
        assert!(snap.resolve(HarnessId::CODEX, &InvocationKey::Name("tdd".into())).is_some());
        let file = InvocationKey::SkillFile(skill.join("SKILL.md"));
        assert_eq!(snap.resolve(HarnessId::CODEX, &file), id);
    }

    #[test]
    fn rollouts_are_found_under_dated_folders_only() {
        let t = tempfile::tempdir().unwrap();
        let day = t.path().join(".codex").join("sessions").join("2026").join("11").join("01");
        fs::create_dir_all(&day).unwrap();
        fs::write(day.join("rollout-2026-11-01T10-00-00-abc.jsonl"), "").unwrap();
        fs::write(day.join("other.jsonl"), "").unwrap();
        fs::write(t.path().join(".codex").join("session_index.jsonl"), "").unwrap();
        let env = Env { home: t.path().to_path_buf(), projects: vec![] };
        let sources = Codex.usage_sources(&env);
        assert_eq!(sources.len(), 1);
        assert!(sources[0].path.ends_with("rollout-2026-11-01T10-00-00-abc.jsonl"));
    }

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

        let env = Env { home: home.to_path_buf(), projects: vec![] };
        let d = Codex.discover(&env, &[]);

        let mut names: Vec<_> = d.bundles.iter().map(|b| (b.name.as_str(), b.enabled)).collect();
        names.sort();
        assert_eq!(names, vec![("b", true), ("off", false), ("r", true)]);
        let builtin = d.entries.iter().filter(|e| e.owner_hint == Ownership::HarnessBuiltin).count();
        assert_eq!(builtin, 2);
        assert!(d.unreadable.is_empty(), "{:?}", d.unreadable);
    }
}
