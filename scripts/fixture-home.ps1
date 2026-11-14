# Builds a fake home for running skilltab with SKILLTAB_HOME, so manage actions never touch real skill folders.
#   pwsh scripts/fixture-home.ps1 -Root target\fixture-home
# The tree holds: a skill junctioned from Claude into ~/.agents (also Codex's), Claude-only real folders,
# a Codex-only skill and a Claude plugin, plus:
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

Skill '.agents\skills\shared-linked' 'shared-linked' 'Real folder in ~/.agents, reached by Claude through a junction.'
Junction '.claude\skills\shared-linked' '.agents\skills\shared-linked'
Skill '.claude\skills\solo-real' 'solo-real' 'Claude-only real folder. Delete it to the Recycle Bin.'
Skill '.claude\skills\notes-real' 'notes-real' 'Claude-only real folder. Disable it onto the shelf and restore it.'
Skill '.agents\skills\codex-only' 'codex-only' 'Only Codex sees this one.'

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
