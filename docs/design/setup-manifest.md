# Setup folder format

A setup folder is a plain folder you keep in git. Export writes it and Restore reads it. Nothing else is stored, and nothing in it is private to skilltab. A script can read or write the same files.

```
skill-setup/
  manifest.json
  config/
    claude/CLAUDE.md
    claude/rules/*.md
    codex/AGENTS.md
  skills/
    <own skill>/SKILL.md
```

## manifest.json

JSON with four lists. A missing list reads as empty, and unknown keys are ignored.

| Key | Holds |
| --- | --- |
| `marketplaces` | Claude Code plugin marketplaces. Each has a `name` and a `source`, which is `{ "source": "github", "repo": "owner/name" }` or `{ "source": "git", "url": "..." }`. Other source types cannot be rebuilt, so Export leaves them out and says so. |
| `claudePlugins` | Plugins to install, as `name@marketplace`. |
| `skillSources` | One entry per source of the `skills` CLI, with the names of the skills it installed from there. |
| `ownSkills` | Folder names under `skills/` in the setup folder. Restore links each one into every harness. |

Export sorts every list and writes two-space indentation, one entry per line, LF line ends and a final newline. Running Export twice gives the same bytes, so a second run has nothing to change.

An empty `marketplaces` or `skillSources` is written as `[]`. An empty `claudePlugins` or `ownSkills` is written as the brackets with one blank line between them. That is odd, and it is what `backup.ps1` writes, because the script passes those two lists through `ForEach-Object`, which hands an empty list on as one null line. Export copies it so a manifest from either tool is the same bytes. Reading accepts both forms.

```json
  "claudePlugins": [

  ],
```

```json
{
  "marketplaces": [
    { "name": "Beta-market", "source": { "source": "github", "repo": "acme/beta" } },
    { "name": "alpha-market", "source": { "source": "git", "url": "https://example.test/acme/alpha.git" } }
  ],
  "claudePlugins": [
    "plug-a@zeta-market",
    "plug-b@alpha-market"
  ],
  "skillSources": [
    { "source": "acme/skills", "skills": ["Upper", "s-one", "s-two"] },
    { "source": "https://example.test", "skills": ["wk"] }
  ],
  "ownSkills": [
    "Own-A",
    "own-b"
  ]
}
```

Every name that becomes a command argument or a path is checked when the manifest is read. It cannot be empty, hold a control character or start with a dash. A folder name cannot hold a separator or a colon, and cannot be `.` or `..`.

## config/

The carried files are an allowlist, one per harness:

| Harness | Machine path | Setup folder path |
| --- | --- | --- |
| Claude Code | `~/.claude/CLAUDE.md` | `config/claude/CLAUDE.md` |
| Claude Code | `~/.claude/rules/*.md` | `config/claude/rules/*.md` |
| Codex | `~/.codex/AGENTS.md` | `config/codex/AGENTS.md` |

Export matches a file on the machine to its copy in the setup folder the way the disk compares names. Where the disk ignores case (Windows and macOS), `Rule.md` and `rule.md` are one file and the copy keeps the spelling it has. On Linux they are two.

Hidden files and folders are skipped, as `backup.ps1` skips them. On Windows that means the Hidden attribute, so `rules/.x.md` is carried. On Linux and macOS it means a name that starts with a dot.

Only markdown is carried. `settings.json`, `config.toml`, `hooks.json`, auth files and sessions are never exported, and a restore ignores them if they appear in the folder. Export refuses to run when a carried file matches one of eight secret patterns, and names the file. The scan reads each file as UTF-8 and as `ReadAllText` would, so a file with a UTF-16 or UTF-32 byte order mark is scanned as the text it holds. It also refuses when `manifest.json`, `config/` or anything under it is a link (a symlink or a Windows junction), because a link in a cloned folder would carry its writes and deletes outside the setup folder.

## What Restore does with it

- It links each own skill into the skills folder of every harness, when that slot is free. An entry under `skills/` that is itself a link is not an own skill. The plan names it and links nothing, because a cloned folder could point it anywhere.
- It copies a config file when the destination is absent. When the destination holds other content, it writes the setup copy beside it as `<name>.from-backup` and tells you to merge by hand. When a `<name>.from-backup` is already there and differs from the setup copy, Restore leaves it alone, because you may have edited it, and the plan names where the setup copy is so you can compare by hand.
- It runs `claude plugin marketplace add`, `claude plugin install` and one `npx skills add` per source, for what is missing. Every command runs with `HOME` and `USERPROFILE` set to the home folder skilltab is using.
- It never replaces a real folder, a link that points elsewhere or a config file that differs.
- It does not sign in. The plan lists that as the last step for you.

## Where Export differs from backup.ps1

Export writes the same manifest and the same config files as `backup.ps1` for a normal machine. These are the differences, and each one is on purpose.

- **Enabled plugins.** The script's `-eq $true` also accepts `1` and `"true"` in `enabledPlugins`. Claude Code reads a real boolean, so a plugin with `1` or `"true"` is not enabled there. skilltab accepts only the JSON boolean `true`, and leaves the other two out.
- **Sources that differ in case.** The script keys a case-insensitive hashtable, so `Acme/Skills` and `acme/skills` are one source. skilltab does the same. The script keeps the spelling of the first skill in the lock's key order. skilltab reads the lock as a parsed object, which does not keep that order, so it keeps the spelling of the first skill by name. The two agree unless the same source is spelled two ways in one lock.
- **A lock entry with no usable source.** An entry with a `sourceUrl` (any type but github) or a `sourceBaseUrl` (well-known) and no `source` is kept and grouped under that url, as the script does. An entry with neither makes the script stop with an error. skilltab leaves the skill out and names it in a note on the plan.
- **A refused or unreadable input.** The script copies every file it can and exits 1 when one is refused. skilltab plans nothing when any file matches a secret pattern or any input cannot be read, so the folder is never left half written.
- **Links.** The script follows a link in the repo folder. skilltab refuses a setup folder where `manifest.json` or anything under `config/` is a link.

Restore has no script to compare with, and it has one rule of its own. A `<name>.from-backup` file that already exists and differs from the setup copy is never replaced, because you may have edited it. Only an absent one is written.

The golden example is `crates/core/tests/fixtures/setup/`. Its `README.md` says how the expected files were made.
