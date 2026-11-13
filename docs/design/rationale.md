# skilltab design rationale (candidate 2: "bodies, mounts, plans")

## Problem

Show every agent skill on a Windows machine across harnesses, with scope, usage, management, and a jstack link. Three facts make the shape non-obvious. First, one physical folder is reached through two or three junction hops (`~/.claude/skills/x` → `~/.agents/skills/x` → plugin cache or jstack), so a list of paths repeats the same skill several times. Removing a hop also breaks every path that runs through it. Second, usage lives in 442 MB of growing transcripts in two different formats. Codex has no Skill tool, so its only signals are `$name` mentions and SKILL.md file reads. Third, several other tools write into the same folders and **put things back**. The measured case is jstack's `sync-skills.ps1`: it runs at every session start and recreates each junction listed in `~/.jstack-links.txt`, which held 61 lines when measured. claude.ai sync rewrites `skills/synced`, and plugin managers own their caches. A delete that ignores the owner gets undone silently.

## Usage (caller's view)

See `usage.md`, which was written before any type. In short, `getInventory()` returns one row per physical skill folder, with its mounts. `getSkill(id)` returns the detail. Every change is `planAction(request)` → show steps, effects and blockers → `applyPlan(planId)`. Adding a harness is one file plus one registry line.

## Shape

**Data structures.** There are two levels of entity, plus the shelf.
- `SkillBody` is a real folder, identified by NTFS file id. It is the row the user sees.
- `Mount` is one harness's view of a body. It holds the harness, scope, entry path, the full `hops: Vec<LinkPath>` chain, invocation keys, owner and state.

`Snapshot` holds both, plus bundles, the shelf and a private index (`through_path`, `by_key`). It is rebuilt whole from disk on every refresh. That takes milliseconds for about 300 entries, so the inventory is never cached or synced (per *single source of truth: derive instead of sync*). Tracing the access patterns through the structure:
- "What breaks if I remove path P?" → `mounts_through(P)`.
- "Which skill did usage key K in harness H mean?" → `resolve(H, K)`.
- "Unused skills" → join `usage.totals()` with `Mount::keys`.
- "Which installed skills came from jstack?" → `jstack.relate(body)`.

None of these needs a structure that is added later.

**Usage is indexed by harness-native key, not by skill.** `usage.db` stores raw events: (harness, `InvocationKey::Name("pstack:architect")` or `SkillFile(path)`, kind, time, session, cwd, dedup id). Keys are matched to bodies only at read time, against the current snapshot. So reinstalls, plugin version bumps and renames need no migration, and usage of skills that have been uninstalled surfaces as **ghosts**. Ingest is incremental per file: (file id, offset, parser state). It reads only appended bytes up to the last newline, uses a `memchr` prefilter before any JSON parse, and runs one rayon task per file. Each source commits its rows and its cursor in one transaction, with `UNIQUE(harness, dedup)` (tool_use id, message uuid, call_id). That makes ingest idempotent, crash-safe, and safe with resumed sessions that copy lines (per *make-operations-idempotent*). The first launch shows the inventory immediately and fills the usage columns when the first pass finishes.

**Delete is made safe by three structural choices, not by care.**
1. *Typed paths.* `LinkPath` and `RealDir` can only be built by `scan`, after an lstat. `Step::RemoveLink` takes a `LinkPath` (RemoveDirectoryW on the reparse point). `Step::Recycle` takes a `RealDir` (IFileOperation to the Recycle Bin). Recycling through a junction, which would empty a folder other harnesses use, does not compile (per *encode-lessons-in-structure*).
2. *Plan, then apply.* `manage::plan(snapshot, action)` is pure and unit-testable without a filesystem. It emits steps, **effects** (for example, "also breaks Claude's mount via ~/.agents/skills/x") and **blockers** that carry a remedy:
   - `RecreatedBy{JstackSync}`: "add x to jstack/exclude.txt"
   - `PartOfBundle`: use the bundle action
   - `SharedBody`: a real folder cannot leave only one harness
   - `InsideJstackRepo`: never recycle the user's own source
   - `HarnessBuiltin`

   `apply` re-checks the plan's preconditions (link still points to T, folder hash unchanged, snapshot revision) and refuses if the plan is stale. Each step is idempotent, and the shelf record is written before the step that needs it. The UI sends only ids that the backend issued, never paths.
3. *One write path.* `manage::win` is private, and bundle and jstack operations are plans too, with a `Run` step. So there is exactly one confirm flow and one place to audit (per *two ways to do one task*).

**Disable is the shelf.** Disabling a mount removes the link and records its target, or stashes a real folder under `%LOCALAPPDATA%\skilltab\shelf`. Restoring relinks or unstashes. Removed links also go to the shelf, so a link deletion can be undone, and folder deletion goes through the Recycle Bin. "Disabled" has one owner (`Shelf`), and the scan merges it in as rows. Bundles use the harness's own CLI (`claude plugin disable ...`) through `Harness::bundle_command`. A harness without one gets an `Unsupported` blocker rather than a hand-edited config file.

**jstack: skilltab reads, jstack acts.** skilltab never re-implements jstack's linking policy and never writes `~/.jstack-links.txt` or the repo. Doing so would give the same state two owners. Install and update run jstack's own commands, `sync-skills.ps1` and `npx skills add <source> -g -a claude-code codex -y --skill ...`, which is the exact line jstack's installer uses, and the plan shows that command before it runs. Relations are derived:
- `Owned`: the body is the repo folder. Drift = uncommitted edits, or behind origin after an on-demand `git fetch`.
- `CopyOf`: same name, separate folder. Drift = content hash.
- `Declared`: listed in `skill-sources.txt`.

**Harness seam.** A harness reports facts (`EntryFact`, `BundleFact`) and parses lines into `RawEvent`s. It knows nothing about links, dedup, provenance, storage or deletion. Adding a harness takes three things: a new submodule, a `HarnessId` const, and one line in `REGISTRY`. The UI reads harness ids and labels from `facets`, and the TS types come from `tauri-specta`, so no list exists twice (per *hand-synced list*).

**Interface depth.** The app crate sees one type, `Skilltab`, with nine methods. The UI sees nine commands and one event. Behind them sit link-graph resolution, file-id dedup, provenance from four record files, an incremental crash-safe indexer over two log formats, a consequence-aware planner, Win32 recycle and junction handling, and git. Filtering is done client-side over about 300 precomputed rows, so the surface has no query language. "Unused" is computed in Rust as `daysSinceLastUse`, so every screen means the same thing by it.

## Synthesis decision

Two runners designed independently, both on Opus. The second seat was configured as `gpt-6-astra`, which Claude Code cannot run, so it ran on the session model. The cross-judge was skipped for the same reason: the pool's only non-Claude entry was not runnable. The base was picked by the orchestrator alone against a six-point rubric (identity vs install, delete safety, usage indexing, harness extensibility, interface depth, jstack on the same identity).

The candidates converged: body rows, per-harness mounts with full junction chains, ownership as a field, usage stored as harness-native keys and resolved at read time, one plan-then-apply door. Convergence is the strongest signal arena gives, so the consensus shape ships.

Base: candidate 2 (this package). It wins on delete safety (`LinkPath`/`RealDir` typed paths make recycling through a junction fail to compile), on idempotent ingest (`UNIQUE` dedup id survives resumed sessions that copy lines), on the bundle scope the user chose (first-class `Bundle`), and on less machinery (no filesystem watcher).

Grafted from candidate 1:
- A per-harness `tracking_since` coverage date on the inventory, so "unused" is never claimed for a period with no transcripts.
- Its rule that the core crate's modules are private (`mod x;`), so the shell cannot import internals.

Rejected from candidate 1:
- Filesystem watchers on harness roots and transcript dirs. Refresh on focus is enough at ~300 entries.
- `Relink` (re-aiming a junction chain so a cut in one harness keeps another). Real need, measured chain, but v1 blocks with `SharedBody` and shows the effect instead. Revisit when a user hits it.
- Allowing a jstack-synced link removal with a "will come back" warning. A change that silently reverts is worse than a refusal with the `exclude.txt` remedy.

Layout deviation: the scaffold already uses Tauri's default `src-tauri/` (app crate) and `src/` (UI). The core goes in `crates/core/`, and the app crate stays at `src-tauri/`. Module boundaries are unchanged.

## Tradeoffs accepted

- We accept a full rescan on every refresh in exchange for no inventory cache to invalidate. At about 300 entries this costs milliseconds. It would need revisiting past roughly 10k skills.
- We accept that history is attributed to a body only while some mount answers to its key. When a plugin version bump moves a Codex `SkillFile` path, the older reads become ghosts. In exchange, stored usage never needs migrating.
- We accept that Codex usage is heuristic (`FileRead` of SKILL.md, `$name` mentions), in exchange for having any Codex signal at all. The UI shows it separately by `kind`.
- We accept refusing to delete links that jstack sync, claude.ai sync, or a plugin manager owns. The remedy is shown instead of making a change that silently reverts.
- We accept shelling out to `git`, `npx skills`, `pwsh sync-skills.ps1` and `claude plugin`, in exchange for zero re-implemented policy. A plan shows the exact command line before it runs.
- We accept client-side filtering, in exchange for one list command and instant filters. That is fine at this scale and would be wrong at 100k rows.
- There is no filesystem watcher. Refresh runs on launch, on window focus, and on demand, which costs about 350 stats. This looks like an oversight but is chosen: a watcher on 442 MB of hot transcripts is more machinery than it pays for.

## Alternatives considered

- **Path-centric inventory (one row per install path, with a "same as" grouping in the UI).** This is shallower. Every caller and every screen has to group paths and work out the consequences of a delete. The fact that one folder sits behind several paths would be spread across the UI.
- **Logical skill = name (group every `tdd` folder across harnesses into one row).** It looks friendlier, but it merges different skills that share a name (claude.ai `docs` and a local `docs`). It also hides the physical fact the delete logic depends on. Name collisions are better shown as a flag on body rows.
- **Usage resolved to SkillId at ingest, with per-skill counters.** The reads would be cheaper, but every reinstall, rename or version move would need a migration or a recount of 442 MB, and ghosts disappear. Resolving at read time is one GROUP BY over tens of thousands of rows, so it costs little.
- **A persistent inventory DB, synced by a filesystem watcher.** This adds a second copy of filesystem truth, with invalidation bugs, and gains nothing at this scale.
- **Direct delete commands (`deleteSkill(path)`) with confirm dialogs in the UI.** It has two ways to change the disk, puts paths in the webview, puts the consequence logic in TS, and leaves no stale-plan check.
- **skilltab manages jstack links itself (writes junctions plus `.jstack-links.txt`).** The link state would have two writers with different rules. jstack's next sync would fight skilltab.

## Open questions and risks

- When a skill is blocked by `RecreatedBy{JstackSync}`, should skilltab offer to append the name to `jstack/exclude.txt` locally? That is a repo edit the user would commit. The current answer is no, show the remedy text.
- Does `claude plugin enable|disable|uninstall|update` exist and behave non-interactively in the installed Claude Code version? If not, bundle actions for Claude become `Unsupported` too.
- Does Claude Code `--resume` copy earlier lines into the new transcript file? Dedup ids make that harmless for counts. The question is whether `uuid` is preserved when lines are copied, which needs checking against a real resumed file.
- Codex project-scope skill folders (`.agents/skills` or `.codex/skills` in a repo) are assumed, not measured. Which is it?
- Project discovery relies on transcript `cwd` values, normalized to their git root. Are there projects with `.claude/skills` that were never opened in a harness? Settings has `extraProjects` for those.
- Should `Disable` of a `SkillsCli`-owned real folder in `~/.agents/skills` warn that `npx skills update` might recreate it?
- Stashing a real folder moves it off-volume if `%LOCALAPPDATA%` is on a different drive from the skill (for example, `D:` projects). The move then becomes copy-and-recycle. Is that acceptable?

## Next implementation step

Write `scan::resolve_entry`, along with `model::Snapshot::mounts_through`, and test them against a temp directory with real NTFS junctions that copy the `.claude/skills → .agents/skills → jstack/skills` chain. Every other module trusts what these produce.
