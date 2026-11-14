# Setup export fixture

`expected/` is the output of the real jstack `backup.ps1 -HomeDir <home>` run against `home/`.
It was produced at jstack commit `6bf3bf2d8f803f5829dfff0497d8b02ce69cd0c8` on 2026-11-14.

- `home/` is a synthetic home folder. It is the only input.
- `expected/manifest.json` and `expected/config/` are what `backup.ps1` wrote.
- `expected/skills/` holds the two own skills that were in the repo folder when it ran. `backup.ps1` lists them in `ownSkills`.

The tests copy `home/` to a temp folder, plan and apply an Export, and compare the written bytes with `expected/`.
`.gitattributes` marks this folder `-text`, so no checkout rewrites line endings.

To regenerate, copy jstack to a scratch folder, put `expected/skills/` at `<scratch>/skills`, and run:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File <scratch>\backup.ps1 -HomeDir <path to home>
```

Then copy `manifest.json` and `config/` from the scratch folder over `expected/`.

## empty-lists/

A second case with no enabled plugins, no skill sources and no own skills. `backup.ps1` writes `[]` for an empty `marketplaces` or `skillSources`, and a blank line between the brackets for an empty `claudePlugins` or `ownSkills`, so this case shows both forms.

- `empty-lists/home/` is the only input. It has one marketplace, one disabled plugin, one `CLAUDE.md` and no skill lock.
- `empty-lists/expected/` is what the real `backup.ps1 -HomeDir <empty-lists/home>` wrote, from a temp copy of jstack commit `6bf3bf2d8f803f5829dfff0497d8b02ce69cd0c8` with no `skills/` folder, on 2026-11-14.
