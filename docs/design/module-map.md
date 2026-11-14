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
| `view` | The IPC types and the pure `model → view` builders (inventory, detail, plan views). `ActionRequest` → `ManageAction` parsing against a snapshot. | the app crate, and through specta the UI | `model`, `usage` (Tally) |
| `model` | Domain types: `Snapshot`, `SkillBody`, `Mount`, `Bundle`, `ShelfEntry`, ids, `LinkPath`, `RealDir`. Snapshot index queries. | crate | nothing (leaf) |
| `harness` (+ `claude`, `codex`) | The **only** per-harness knowledge: where skills are, how they are named, how usage lines look, which CLI manages bundles. `REGISTRY` is the single list of harnesses. | crate: trait, `registry()`, fact types. The submodules are private. | `model` (ids, keys, scope, ownership) |
| `scan` | Turns `EntryFact`s into a `Snapshot`: link resolution, body dedup by file id, frontmatter, content hash, shadowing, index. **The sole constructor of `LinkPath`, `RealDir`, `SkillId`, `MountId`.** It also re-checks those facts for apply. | crate | `model`, `harness`, `provenance` |
| `provenance` | Reads other tools' records: `.skill-lock.json` and synced manifests. Answers origin and owner. It writes nothing. | crate | `model` |
| `usage` | `usage.db`: schema, byte cursors, ingest, aggregates, known projects. The only persisted usage. | crate | `harness` (parsers), `model` (keys) |
| `manage` | `plan` (pure), `apply` (impure), the `Shelf` (`shelf.json` plus stash folders), and the private `win` and `unix` twins (unlink, link, recycle), one of which `cfg` picks. **The only module that changes the disk.** | crate | `model`, `scan` (re-check), `harness` (bundle commands) |

Rules that keep it this way:

- `model` types are not `Serialize`. Only `view` types cross IPC. Wire shape changes stay in `view`.
- `LinkPath` and `RealDir` have private fields and only `scan` constructs them, so `recycle` cannot be handed a link.
- `manage::win` and `manage::unix` are private submodules with the same functions (`remove_link`, `create_link`, `recycle`) and a `TRASH_NAME` const. `manage.rs` picks one with `cfg` and calls it `sys`. The only way to change the disk is `Skilltab::apply(plan_id)`, and a plan id comes only from `plan`.
- `cfg(windows)` and `cfg(unix)` live in `manage` (the twins and `SystemOs::run`), `fsutil` (`file_id`), `model::norm_key`, `harness::codex` (`canonical_skill_file`), `lib.rs` (`reveal_in_explorer`) and tests. Everything else is one code path on every OS. A new `cfg` anywhere else needs a reason here.
- Tests make links through `testutil::link`, which calls the same `create_link` that `apply` uses.
- A harness submodule never touches SQLite, links or provenance. It emits facts and events.
- Call depth for any operation: `commands.rs` → `Skilltab` method → one domain module (plus `scan` for re-checks). That is three files at most.

## skilltab (`src-tauri`)

| File | Owns |
| --- | --- |
| `commands.rs` | Ten one-line commands, one event, the `tauri_specta::Builder` (the single command list that drives both the handler and `bindings.ts`). |
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
| Shelf, broken links, ghosts tabs | Fields of `Inventory`. Restore and remove go through the action dialog. |

## Persisted state, one owner each

| File (in the data folder) | Owner |
| --- | --- |
| `usage.db` | `usage` |
| `aliases.json` | `aliases` |
| `shelf.json`, `shelf/<id>/` | `manage::Shelf` |

The data folder is Tauri's local data directory plus `skilltab`: `%LOCALAPPDATA%\skilltab`, `~/.local/share/skilltab` or `~/Library/Application Support/skilltab`.

Nothing else is stored. The inventory is rebuilt from disk on every refresh.
