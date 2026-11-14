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

Only markdown is carried. `settings.json`, `config.toml`, `hooks.json`, auth files and sessions are never exported, and a restore ignores them if they appear in the folder. Export refuses to run when a carried file matches one of eight secret patterns, and names the file. It also refuses when `manifest.json`, `config/` or anything under it is a link (a symlink or a Windows junction), because a link in a cloned folder would carry its writes and deletes outside the setup folder.

## What Restore does with it

- It links each own skill into the skills folder of every harness, when that slot is free.
- It copies a config file when the destination is absent. When the destination holds other content, it writes the setup copy beside it as `<name>.from-backup` and tells you to merge by hand.
- It runs `claude plugin marketplace add`, `claude plugin install` and one `npx skills add` per source, for what is missing. Every command runs with `HOME` and `USERPROFILE` set to the home folder skilltab is using.
- It never replaces a real folder, a link that points elsewhere or a config file that differs.
- It does not sign in. The plan lists that as the last step for you.

The golden example is `crates/core/tests/fixtures/setup/`. Its `README.md` says how the expected files were made.
