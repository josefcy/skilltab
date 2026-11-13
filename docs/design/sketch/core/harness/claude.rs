//! Claude Code.
//! Skills:   ~/.claude/skills/<name>, ~/.claude/skills/synced/<id>/<name> (claude.ai, named "anthropic-skills:<name>"),
//!           <installPath>/skills/<name> per installed_plugins.json v2 (named "<plugin>:<name>"),
//!           <project>/.claude/skills/<name>.
//! Usage:    ~/.claude/projects/<slug>/*.jsonl and <slug>/<session>/subagents/*.jsonl.
//!           Model call:  assistant content item {"type":"tool_use","name":"Skill","input":{"skill":"<key>"}}
//!           Slash call:  user text containing <command-name>/<key></command-name>
//!           Every line has "timestamp", "cwd", "sessionId", "uuid".

use super::*;

pub(super) struct ClaudeCode;

impl Harness for ClaudeCode {
    fn id(&self) -> HarnessId { HarnessId::CLAUDE_CODE }
    fn label(&self) -> &'static str { "Claude Code" }

    fn discover(&self, env: &Env, projects: &[ProjectRoot]) -> Discovery {
        // TODO
        // user:    scan_skill_dir(~/.claude/skills, User, Unmanaged, Plain)
        // synced:  for each synced/<id>/manifest.json -> scan_skill_dir(synced/<id>, User, ClaudeAiSync, Prefixed("anthropic-skills"))
        // plugins: parse installed_plugins.json, read enabledPlugins from settings.json (user and project),
        //          one BundleFact per "<plugin>@<mkt>" entry, members from plugin.json "skills" or <installPath>/skills
        //          entries: scope Bundle{key}, owner PluginManager, naming Prefixed(<plugin>)
        // project: for p in projects -> scan_skill_dir(p/.claude/skills, Project(p), Unmanaged, Plain)
        unimplemented!()
    }

    fn usage_sources(&self, env: &Env) -> Vec<UsageSource> { unimplemented!() } // append_only: true

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
