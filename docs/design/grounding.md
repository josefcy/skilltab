# skilltab: grounding for the design sketch

## User goal

A Windows desktop app for a developer that shows every agent skill installed on this machine, across harnesses (Claude Code and Codex today, more later), with each skill's scope, how often it is used, and controls to delete and manage it. It also links to the user's own skill repo, jstack.

Problem being solved: skills are scattered over user, project, and plugin folders per harness, so nothing shows what is installed, where it applies, or what goes unused.

## Decisions the user already made

- Stack: Tauri 2. Rust core, React + TypeScript UI in WebView2.
- Scopes shown in the UI: user (machine-wide for that user), project (a repo's own skills folder), and plugin bundle (skills that arrive together as one plugin or pack and are managed as a unit).
- jstack link, first version: show which installed skills come from jstack, flag when the installed copy differs from the repo, install and update skills from it. No pushing changes back.
- Greenfield. `D:\skilltab` is empty. No existing code to integrate with.

## Facts measured on this machine (Windows 11, user home `C:\Users\josef`)

Toolchain: rustc/cargo 1.99.1 (stable-x86_64-pc-windows-msvc), Node 24.20, pnpm 12.15, WebView2 runtime 157, git, gh. No Tauri CLI installed yet.

Skill is a folder holding `SKILL.md` with YAML frontmatter (`name`, `description`, sometimes `disable-model-invocation`).

Claude Code:
- User skills: `~/.claude/skills/<name>/SKILL.md`. About 55 entries. Every one is an NTFS junction to `~/.agents/skills/<name>`.
- `~/.claude/skills/synced/<uuid>_<uuid>/` holds skills synced from claude.ai. `~/.claude/skills/.trash` exists.
- Plugins: `~/.claude/plugins/installed_plugins.json` (version 2) maps `"<plugin>@<marketplace>"` to entries with `scope` ("user" or "project"), `installPath`, `version`, `gitCommitSha`. Plugin skills live at `<installPath>/skills/<name>/SKILL.md`. 134 SKILL.md files under `~/.claude/plugins/cache`. `~/.claude/settings.json` has `enabledPlugins`.
- Project skills: `<project>/.claude/skills/<name>/SKILL.md`. Known projects can be found from `~/.claude/projects/<slug>/` (15 dirs). The slug is the cwd with `:`, `\`, `/`, `.` turned into `-`, which is lossy. Transcript lines carry a real `cwd` field.
- Usage data: `~/.claude/projects/<slug>/*.jsonl` session transcripts, 354 files, 442 MB total and growing. A slash invocation appears in a user message as `<command-name>/pstack:poteto-mode</command-name>`. A model-initiated invocation is an assistant `tool_use` with `name: "Skill"` and `input.skill`. Plugin skills are named `plugin:skill`.

Cross-harness store (`skills` CLI from vercel-labs):
- `~/.agents/skills/` has 101 entries. 55 are junctions into the pstack plugin cache (`~/.claude/plugins/cache/cursor-plugins/pstack/0.15.36/skills`). 3 are junctions into `~/jstack/skills`. The rest are real folders.
- `~/.agents/.skill-lock.json` (version 3) maps skill name to `source`, `sourceType`, `sourceUrl`, `ref`, `skillPath`, `skillFolderHash`, `installedAt`, `updatedAt`.

Codex:
- `~/.codex/skills/` holds `graphify` and `.system/` (imagegen, openai-docs, review-agent, skill-creator, skill-installer).
- `~/.codex/sessions/` holds 190 session files. Format not yet inspected.

Other harness dirs present with no skills folder: `~/.copilot`, `~/.orca`, `~/.t3`.

jstack: local clone at `C:\Users\josef\jstack` with `origin https://github.com/josefcy/jstack.git`. Skills at `jstack/skills/<name>/`.

## Consequences the design must handle

- One physical skill folder is reachable through several install paths. Example chain: `~/.claude/skills/tdd` (junction) -> `~/.agents/skills/tdd` (real, or another junction) -> plugin cache or jstack. Listing by path alone shows the same skill two or three times.
- Delete means different things: removing a junction (the skill stays on disk and in other harnesses) versus removing the real folder (every junction to it breaks). Deletes go to the Recycle Bin. Deleting a plugin-bundle skill individually fights the plugin manager, so the bundle is the unit there.
- Usage counting must not reparse 442 MB on every launch.
- Adding a future harness should be a small, local change.
- The UI needs: a filterable inventory (by harness, scope, source, unused), a skill detail view (SKILL.md, install paths, usage over time), and manage actions (delete, enable or disable, open folder, install or update from jstack).

## What to produce

Write into your own output directory only:

- `usage.md`: README-style usage plus two or three real call sites, written before the types.
- Sketch files: Rust types and trait or function signatures with `unimplemented!()` bodies, the Tauri command surface, and the TypeScript types the UI consumes.
- `module-map.md`: modules, what each owns, who may import what.
- `rationale.md`: shaped per the rationale template named in your prompt, including alternatives considered and rejected.

Do not scaffold a buildable project. Do not write outside your output directory. Do not modify anything under the user's home.
