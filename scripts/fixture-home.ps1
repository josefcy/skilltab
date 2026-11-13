# Builds a fake home for running skilltab with SKILLTAB_HOME, so manage actions never touch real skill folders.
#   pwsh scripts/fixture-home.ps1 -Root target\fixture-home
# The tree holds: a skill junctioned from Claude into ~/.agents (also Codex's), Claude-only real folders,
# a Codex-only skill, a Claude plugin, and the jstack pieces:
#   jstack\            a git clone of a local bare remote that is one commit ahead (fetched, not pulled), so the jstack
#                      tab shows "behind 1" and Pull has work. It holds an owned skill (from-jstack, linked and listed in
#                      .jstack-links.txt), a skill with a hand copy of different content (copy-drift), a skill the plugin
#                      already names (architect), one nobody has (not-installed-yet), a stub sync-skills.ps1 that only
#                      prints, and a skill-sources.txt line with nothing installed.
#   .claude\projects   a transcript whose Skill calls use an old plugin prefix, so one alias suggestion appears.
# Nothing here touches the real home. Point the app at it with SKILLTAB_HOME.
param([Parameter(Mandatory = $true)][string]$Root)
$ErrorActionPreference = 'Stop'

$full = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $Root))
if ($full -notmatch 'fixture') { throw "Refusing: the fixture root must have 'fixture' in its path ($full)." }
if ($full.TrimEnd('\') -ieq $env:USERPROFILE.TrimEnd('\')) { throw "Refusing: $full is the real home." }
if (Test-Path $full) {
    # Junctions are removed as links first, so Remove-Item never walks into a target.
    Get-ChildItem $full -Recurse -Force -Attributes ReparsePoint | ForEach-Object { [System.IO.Directory]::Delete($_.FullName) }
    Remove-Item $full -Recurse -Force
}

function Skill([string]$rel, [string]$name, [string]$desc) {
    $dir = Join-Path $full $rel
    New-Item -ItemType Directory -Force $dir | Out-Null
    Set-Content -Encoding utf8 (Join-Path $dir 'SKILL.md') "---`nname: $name`ndescription: $desc`n---`n# $name`n`nFixture skill for skilltab's manage flow.`n"
}
function Junction([string]$link, [string]$target) {
    $l = Join-Path $full $link
    New-Item -ItemType Directory -Force (Split-Path $l) | Out-Null
    New-Item -ItemType Junction -Path $l -Target (Join-Path $full $target) | Out-Null
}
function InRepo([string]$dir, [string[]]$gitArgs) {
    & git -C $dir -c user.name=fixture -c user.email=fixture@example.invalid -c commit.gpgsign=false -c core.autocrlf=false @gitArgs
    if ($LASTEXITCODE -ne 0) { throw "git $gitArgs failed in $dir" }
}
function RepoSkill([string]$repo, [string]$name, [string]$body) {
    $dir = Join-Path $repo "skills\$name"
    New-Item -ItemType Directory -Force $dir | Out-Null
    Set-Content -Encoding utf8 (Join-Path $dir 'SKILL.md') "---`nname: $name`ndescription: Fixture jstack skill $name.`n---`n# $name`n`n$body`n"
}

Skill '.agents\skills\shared-linked' 'shared-linked' 'Real folder in ~/.agents, reached by Claude through a junction.'
Junction '.claude\skills\shared-linked' '.agents\skills\shared-linked'
Skill '.claude\skills\solo-real' 'solo-real' 'Claude-only real folder. Delete it to the Recycle Bin.'
Skill '.claude\skills\notes-real' 'notes-real' 'Claude-only real folder. Disable it onto the shelf and restore it.'
Skill '.agents\skills\codex-only' 'codex-only' 'Only Codex sees this one.'

$seed = Join-Path $full '.fixture-origin\seed'
$bare = Join-Path $full '.fixture-origin\origin.git'
$clone = Join-Path $full 'jstack'
New-Item -ItemType Directory -Force $seed | Out-Null
InRepo $seed @('init', '-q', '-b', 'main')
RepoSkill $seed 'from-jstack' 'Version one, from the first commit.'
RepoSkill $seed 'copy-drift' 'The repo version of this skill.'
RepoSkill $seed 'architect' 'The repo skill the pstack plugin also names.'
RepoSkill $seed 'not-installed-yet' 'No harness has this one.'
Set-Content -Encoding utf8 (Join-Path $seed 'sync-skills.ps1') "# Fixture stub. The real sync-skills.ps1 writes to the real home, so this one only prints.`nWrite-Output 'fixture sync-skills: nothing was changed'"
Set-Content -Encoding utf8 (Join-Path $seed 'skill-sources.txt') "# Fixture sources: nothing of this is installed.`nowner/fixture-pack alpha-tool beta-tool"
InRepo $seed @('add', '-A')
InRepo $seed @('commit', '-q', '-m', 'first commit')
InRepo $full @('clone', '-q', '--bare', $seed, $bare)
InRepo $full @('clone', '-q', $bare, $clone)
RepoSkill $seed 'from-jstack' 'Version two. This line only exists after the pull.'
RepoSkill $seed 'fresh-from-upstream' 'A skill that arrives with the pull.'
InRepo $seed @('add', '-A')
InRepo $seed @('commit', '-q', '-m', 'second commit: new body and a new skill')
InRepo $seed @('push', '-q', $bare, 'main')
InRepo $clone @('fetch', '-q')

Skill '.claude\skills\copy-drift' 'copy-drift' 'A hand copy of the jstack skill, with other content.'
Junction '.claude\skills\from-jstack' 'jstack\skills\from-jstack'
Set-Content -Encoding utf8 (Join-Path $full '.jstack-links.txt') (Join-Path $full '.claude\skills\from-jstack')

$plugin = Join-Path $full '.claude\plugins\cache\cursor-plugins\pstack\1.0.0'
Skill '.claude\plugins\cache\cursor-plugins\pstack\1.0.0\skills\architect' 'architect' 'Ships inside the pstack plugin.'
$installed = @{ version = 2; plugins = @{ 'pstack@cursor-plugins' = @(@{ scope = 'user'; installPath = $plugin; version = '1.0.0' }) } }
$installed | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 (Join-Path $full '.claude\plugins\installed_plugins.json')
'{"enabledPlugins":{"pstack@cursor-plugins":true}}' | Set-Content -Encoding utf8 (Join-Path $full '.claude\settings.json')

$projects = Join-Path $full '.claude\projects\D--fixture-project'
New-Item -ItemType Directory -Force $projects | Out-Null
$when = (Get-Date).ToUniversalTime().AddDays(-2)
$lines = foreach ($i in 1..3) {
    @{
        type = 'assistant'; uuid = "a-$i"; sessionId = 'fixture'; cwd = 'D:\fixture-project'
        timestamp = $when.AddMinutes($i).ToString('yyyy-MM-ddTHH:mm:ss.000Z')
        message = @{ content = @(@{ type = 'tool_use'; id = "toolu_$i"; name = 'Skill'; input = @{ skill = 'mattpocock-skills:notes-real' } }) }
    } | ConvertTo-Json -Depth 6 -Compress
}
# Windows PowerShell's utf8 writes a BOM, which would spoil the first JSON line.
[System.IO.File]::WriteAllLines((Join-Path $projects 'session.jsonl'), [string[]]$lines, (New-Object System.Text.UTF8Encoding($false)))

Write-Output "fixture home: $full"
