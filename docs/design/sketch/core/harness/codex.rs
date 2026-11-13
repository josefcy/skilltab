//! Codex.
//! Skills:   ~/.agents/skills/<name> (the shared store; Codex loads it directly),
//!           ~/.codex/skills/<name>, ~/.codex/skills/.system/<name> (HarnessBuiltin),
//!           ~/.codex/plugins/cache/<mkt>/<plugin>/<ver>/skills/<name>, enabled per config.toml [plugins."<p>@<mkt>"];
//!           marketplaces openai-bundled and openai-primary-runtime ship with Codex, so their bundles are HarnessBuiltin.
//! Usage:    ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
//!           line 0: {"type":"session_meta","payload":{"id","cwd",...}}
//!           Codex has no Skill tool. A skill is "used" when a response_item custom_tool_call/function_call
//!           input names a path ending in \skills\<x>\SKILL.md (FileRead), or a user message has $<name> (Mention).
//!           The "## Skills" listing in developer messages is availability, not use, and is ignored.

use super::*;

pub(super) struct Codex;

impl Harness for Codex {
    fn id(&self) -> HarnessId { HarnessId::CODEX }
    fn label(&self) -> &'static str { "Codex" }

    fn discover(&self, env: &Env, projects: &[ProjectRoot]) -> Discovery {
        // TODO user: ~/.agents/skills and ~/.codex/skills (Plain); .system (HarnessBuiltin)
        //      plugins: newest version dir per <mkt>/<plugin> in the cache; enabled from config.toml
        //      project: p/.agents/skills, p/.codex/skills
        unimplemented!()
    }

    fn usage_sources(&self, env: &Env) -> Vec<UsageSource> { unimplemented!() } // append_only: true

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
