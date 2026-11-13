# Module map

Two crates and one UI package.

```
skilltab/
  crates/core/        skilltab-core   plain Rust lib, no Tauri. Holds every decision.
  crates/app/         skilltab-app    Tauri 2 shell: commands, events, window, capability file.
  ui/                 React + TS      reads src/bindings.ts (generated) only.
```

## skilltab-core

| Module | Owns | Public to | May import |
| --- | --- | --- | --- |
| `lib` (`Skilltab`) | Runtime state: the current `Arc<Snapshot>`, kept plans, the writer lock. Runs refresh, which is ingest, then provenance plus jstack load, then scan. | the app crate (the only `pub` item besides `view`, `Env`, `Settings`) | every module below |
| `view` | The IPC types and the pure `model → view` builders (inventory, detail, plan views). `ActionRequest` → `ManageAction` parsing against a snapshot. | the app crate, and through specta the UI | `model`, `usage` (Tally), `jstack` (relations) |
| `model` | Domain types: `Snapshot`, `SkillBody`, `Mount`, `Bundle`, `ShelfEntry`, ids, `LinkPath`, `RealDir`. Snapshot index queries. | crate | nothing (leaf) |
| `harness` (+ `claude`, `codex`) | The **only** per-harness knowledge: where skills are, how they are named, how usage lines look, which CLI manages bundles. `REGISTRY` is the single list of harnesses. | crate: trait, `registry()`, fact types. The submodules are private. | `model` (ids, keys, scope, ownership) |
| `scan` | Turns `EntryFact`s into a `Snapshot`: link resolution, body dedup by file id, frontmatter, content hash, shadowing, index. **The sole constructor of `LinkPath`, `RealDir`, `SkillId`, `MountId`.** It also re-checks those facts for apply. | crate | `model`, `harness`, `provenance` |
| `provenance` | Reads other tools' records: `.skill-lock.json`, `~/.jstack-links.txt`, synced manifests. Answers origin and owner. It writes nothing. | crate | `model` |
| `usage` | `usage.db`: schema, byte cursors, ingest, aggregates, known projects. The only persisted usage. | crate | `harness` (parsers), `model` (keys) |
| `jstack` | The repo model (`git` CLI), relations and drift (pure), command lines for jstack's own scripts. It never writes the repo or the jstack links file. | crate | `model`, `harness::ExternalCommand` |
| `manage` | `plan` (pure), `apply` (impure), the `Shelf` (`shelf.json` plus stash folders), private `win` (Win32 unlink, recycle, junction). **The only module that changes the disk.** | crate | `model`, `scan` (re-check), `harness` (bundle commands), `jstack` (commands) |

Rules that keep it this way:

- `model` types are not `Serialize`. Only `view` types cross IPC. Wire shape changes stay in `view`.
- `LinkPath` and `RealDir` have private fields and only `scan` constructs them, so `manage::win::recycle` cannot be handed a junction.
- `manage::win` is a private submodule. The only way to change the disk is `Skilltab::apply(plan_id)`, and a plan id comes only from `plan`.
- A harness submodule never touches SQLite, links or provenance. It emits facts and events.
- Call depth for any operation: `commands.rs` → `Skilltab` method → one domain module (plus `scan` for re-checks). That is three files at most.

## skilltab-app

| File | Owns |
| --- | --- |
| `commands.rs` | Nine one-line commands, one event, the `tauri_specta::Builder` (the single command list that drives both the handler and `bindings.ts`). |
| `main.rs` | Builds `Env` and `Settings` (`%APPDATA%\skilltab\settings.json`), runs `Skilltab::open`, starts the first refresh in the background, refreshes on window focus. |
| `capabilities/default.json` | Allows the generated commands and `core:event:default`. No fs, shell or opener plugin. `reveal` goes through the core. |

## ui

| Area | Uses |
| --- | --- |
| `bindings.ts` (generated) | Everything. The UI never declares backend types by hand. |
| Inventory table | `getInventory`, with client-side filters over `facets` and fields of `SkillRow`. Re-fetches when `revision` changes. |
| Detail pane | `getSkill`, rendered `skillMd`, a mount chain diagram from `MountDetail.chain`, and a usage chart from `series`. |
| Action dialog (one component for every action) | `planAction` → render steps, effects and blockers → `applyPlan`. It is the only place that calls `applyPlan`. |
| jstack panel | `getJstack(fetch)`, with install and update buttons that send `{ kind: "jstack", op }` to the same action dialog. |
| Shelf, broken links, ghosts tabs | Fields of `Inventory`. Restore and remove go through the action dialog. |

## Persisted state, one owner each

| File (`%LOCALAPPDATA%\skilltab\`) | Owner |
| --- | --- |
| `usage.db` | `usage` |
| `shelf.json`, `shelf\<id>\` | `manage::Shelf` |
| `%APPDATA%\skilltab\settings.json` | `lib` |

Nothing else is stored. The inventory is rebuilt from disk on every refresh.
