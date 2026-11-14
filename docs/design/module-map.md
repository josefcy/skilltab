# Module map

Two crates and one UI package.

```
skilltab/
  crates/core/        skilltab-core   plain Rust lib, no Tauri. Holds every decision.
  src-tauri/          skilltab        Tauri 2 shell: commands, events, window, capability file.
  src/                React + TS      the UI. Reads src/bindings.ts (generated) only.
```

## skilltab-core

| Module | Owns | Public to | May import |
| --- | --- | --- | --- |
| `lib` (`Skilltab`) | Runtime state: the current `Arc<Snapshot>`, kept plans, the writer lock. Runs refresh, which is ingest, then provenance, then scan. | the app crate (`Skilltab`, `Env` and the `view` module are all it uses) | every module below |
| `view` | The IPC types and the pure `model → view` builders (inventory, detail, plan views). `ActionRequest` → `ManageAction` parsing against a snapshot and `Env`. It is the one place a request becomes a `ManageAction`, so it is also where `setup::read_export` and `setup::read_restore` run. It takes the `Os`, because a restore asks it which tools exist. | the app crate, and through specta the UI | `model`, `usage` (Tally), `setup` |
| `model` (+ `model/setup.rs`) | Domain types: `Snapshot`, `SkillBody`, `Mount`, `Bundle`, `ShelfEntry`, ids, `LinkPath`, `RealDir`. Snapshot index queries. `model/setup.rs` holds `SetupManifest` and the validated strings `Arg` and `FolderName`. | crate | nothing (leaf) |
| `harness` (+ `claude`, `codex`) | The **only** per-harness knowledge: where skills are, how they are named, how usage lines look, which CLI manages bundles, and its part in a setup backup. `setup()` is static data (config folder, the allowlist of carried files, user skills folder, skills CLI agent id, tools, install hint, sign-in text), `read_plugins()` reads its plugin records, and `plugin_ops(want, have)` says per marketplace and plugin whether it is `Present` or `Missing(ExternalCommand)`. `REGISTRY` is the single list of harnesses. | crate: trait, `registry()`, fact types. The submodules are private. | `model` (ids, keys, scope, ownership) |
| `scan` | Turns `EntryFact`s into a `Snapshot`: link resolution, body dedup by file id, frontmatter, content hash, shadowing, index. **The sole constructor of `LinkPath`, `RealDir`, `SkillId`, `MountId`.** It also re-checks those facts for apply. | crate | `model`, `harness`, `provenance` |
| `provenance` | Reads other tools' records: `.skill-lock.json` and synced manifests. Answers origin and owner, and groups the lock's skills by install source (`skill_sources`). It is the only reader of the lock. It writes nothing. | crate | `model` |
| `setup` | The setup backup format and the reading for an export. `SetupDir::parse` (the one place a folder string from the UI becomes a path), `SetupManifest::parse` and `render` (byte for byte what jstack's `backup.ps1` writes), `find_secret` (its eight patterns), `ConfigFile`, `read_export`, which gathers an `ExportInput` of plain data, and `read_restore` (in `setup/restore.rs`), which gathers a `RestoreInput`. `Wanted` is the one list of machine paths a restore asks about: the own skill link slots, the skill slots of each skills CLI source, and each config destination with its `.from-backup` sibling. `Seen` is read from exactly that list and says per path whether it is `Absent`, `LinkTo(target)` or `Other`, plus the bytes of the config files. A path nobody read is `Other`, so it is never overwritten. **It writes nothing.** | `view`, `manage` | `model`, `harness`, `provenance` |
| `usage` | `usage.db`: schema, byte cursors, ingest, aggregates, known projects. The only persisted usage. | crate | `harness` (parsers), `model` (keys) |
| `manage` | `plan` (pure), `apply` (impure), the `Shelf` (`shelf.json` plus stash folders), the private `win` and `unix` twins (unlink, link, recycle), one of which `cfg` picks, the `Os` seam (`recycle`, `run`, `find_tool`, with `SystemOs` and a fake in tests), and the private `setup` planner (`manage/setup.rs`, pure: an `ExportInput` or a `RestoreInput` in, steps, notes and blockers out). **The only module that changes the disk.** | crate | `model`, `scan` (re-check), `harness` (bundle commands), `setup` (input types) |

Rules that keep it this way:

- `model` types are not `Serialize`. Only `view` types cross IPC. Wire shape changes stay in `view`.
- The UI names things by ids the core issued, never by path. The one exception is the setup folder: `ActionRequest::ExportSetup { folder }` and `RestoreSetup { folder }` carry a string, and `parse_action` turns it into a `SetupDir` (absolute after `~` expansion, existing, canonical) before anything else sees it. Nothing stores the folder in core. The UI remembers it.
- Steps are flat: `RemoveLink`, `Recycle`, `Stash`, `Link`, `Unstash`, `Run`, `Write`, `Delete`. `Link` is both a first link and the shelf restore (`shelf: Option<ShelfId>`). `Write { to, bytes, was }` opens with `create_new` over `Was::Absent` and goes through a temp file and a rename over `Was::Hash`. `Delete { path, was }` removes a file only while it still has the planned content. `stale_reason` refuses the whole plan when a `was` no longer holds.
- A restore plans local steps first (links, config files), then network `Run` steps (marketplaces, plugins, one `npx skills add` per source), so a failed download cannot strand the local work. It plans only what is missing at plan time, so `Step::Run` can stay never-done, and the plan after a finished restore has no steps. A real folder, a link to another target or a differing config file is never replaced: the repo copy of a config file goes to `<dest>.from-backup` when that is absent, and a note says so. An existing `.from-backup` that differs is never replaced, and the note names the repo copy. A config file the allowlist does not carry, or one that is a link or sits behind one, is named in a note and never written. Every `Run` of a restore carries `HOME` and `USERPROFILE` set to `Env.home`, and a program it runs that `Os::find_tool` cannot find is a `Blocker::Unsupported` with an install remedy. `git` is not checked and `node` is implied by `npx`.

A plan has `notes` of kind Present, Attention and Manual. A present item has no step, so `ready` keeps its rule (at least one step, no blockers) and a plan that is all notes reads "Nothing to change".
- A harness never returns a `Step`. It returns facts (`HarnessSetup`, `PluginRecord`) and `ExternalCommand`s, and the planner never names a harness id. The allowlist of carried files lives only in `HarnessSetup::carry`, and a test asserts every entry is markdown, so `settings.json`, `config.toml`, `hooks.json` and auth files are never carried.
- `LinkPath` and `RealDir` have private fields and only `scan` constructs them, so `recycle` cannot be handed a link.
- `manage::win` and `manage::unix` are private submodules with the same functions (`remove_link`, `create_link`, `recycle`) and a `TRASH_NAME` const. `manage.rs` picks one with `cfg` and calls it `sys`. The only way to change the disk is `Skilltab::apply(plan_id)`, and a plan id comes only from `plan`.
- `cfg(windows)` and `cfg(unix)` live in `manage` (the twins, `SystemOs::run`, and the twins' `tool_names`, `is_executable` and `extra_tool_dirs` that `SystemOs::find_tool` and `run` share, so a tool `find_tool` sees is one `run` finds), `fsutil` (`file_id`, `is_hidden`), `model::norm_key` and `model::fold_name`, `harness::codex` (`canonical_skill_file`), `lib.rs` (`reveal_in_explorer`) and tests. Everything else is one code path on every OS. A new `cfg` anywhere else needs a reason here.
- Tests make links through `testutil::link`, which calls the same `create_link` that `apply` uses.
- A harness submodule never touches SQLite, links or provenance. It emits facts and events.
- Call depth for any operation: `commands.rs` → `Skilltab` method → one domain module (plus `scan` for re-checks). That is three files at most.

## skilltab (`src-tauri`)

| File | Owns |
| --- | --- |
| `commands.rs` | Ten one-line commands (no setup command: `plan_action` carries any request), one event, the `tauri_specta::Builder` (the single command list that drives both the handler and `bindings.ts`). |
| `main.rs` | Calls `skilltab_lib::run`. |
| `lib.rs` | Builds `Env` and the data folder (`SKILLTAB_HOME`, `SKILLTAB_DATA`, else Tauri's local data dir plus `skilltab`), runs `Skilltab::open`, refreshes on window focus. In a debug build it also regenerates `src/bindings.ts`. |
| `capabilities/default.json` | Allows the generated commands and `core:event:default`. No fs, shell or opener plugin. `reveal` goes through the core. |

## src (the UI)

| Area | Uses |
| --- | --- |
| `bindings.ts` (generated) | Everything. The UI never declares backend types by hand. |
| Inventory table | `getInventory`, with client-side filters over `facets` and fields of `SkillRow`. Re-fetches when `revision` changes. |
| Detail pane | `getSkill`, rendered `skillMd`, a mount chain diagram from `MountDetail.chain`, and a usage chart from `series`. |
| Action dialog (one component for every action) | `planAction` → render steps, effects and blockers → `applyPlan`. It is the only place that calls `applyPlan`. |
| Bulk selection | A set of row ids in `App`, pruned on every rescan. The bar above the table sends one `{ kind: "deleteBodies", skills }` to the action dialog, which lists the plan's `skipped` skills apart from its steps. |
| Setup tab (`SetupPanel`) | One folder field, remembered per viewer in `localStorage`. Export… and Restore… send `exportSetup` or `restoreSetup` with that folder to the action dialog. The dialog shows the plan's notes in three blocks and keeps the attention and sign-in notes after an apply. |
| Shelf, broken links, ghosts tabs | Fields of `Inventory`. Restore and remove go through the action dialog. |

## Persisted state, one owner each

| File (in the data folder) | Owner |
| --- | --- |
| `usage.db` | `usage` |
| `aliases.json` | `aliases` |
| `shelf.json`, `shelf/<id>/` | `manage::Shelf` |

The data folder is Tauri's local data directory plus `skilltab`: `%LOCALAPPDATA%\skilltab`, `~/.local/share/skilltab` or `~/Library/Application Support/skilltab`.

Nothing else is stored. The inventory is rebuilt from disk on every refresh.
