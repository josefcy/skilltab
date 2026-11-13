# Builds a fake home for running skilltab with SKILLTAB_HOME, so manage actions never touch real skill folders.
#   pwsh scripts/fixture-home.ps1 -Root target\fixture-home
# The tree holds: a skill junctioned from Claude into ~/.agents (also Codex's), Claude-only real folders,
# a jstack repo skill whose Claude link is listed in .jstack-links.txt, a Codex-only skill, and a Claude plugin.
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
Skill 'jstack\skills\from-jstack' 'from-jstack' 'Lives in the jstack clone; its link is recreated by jstack sync.'
Junction '.claude\skills\from-jstack' 'jstack\skills\from-jstack'
Set-Content -Encoding utf8 (Join-Path $full '.jstack-links.txt') (Join-Path $full '.claude\skills\from-jstack')

$plugin = Join-Path $full '.claude\plugins\cache\cursor-plugins\pstack\1.0.0'
Skill '.claude\plugins\cache\cursor-plugins\pstack\1.0.0\skills\architect' 'architect' 'Ships inside the pstack plugin.'
$installed = @{ version = 2; plugins = @{ 'pstack@cursor-plugins' = @(@{ scope = 'user'; installPath = $plugin; version = '1.0.0' }) } }
$installed | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 (Join-Path $full '.claude\plugins\installed_plugins.json')
'{"enabledPlugins":{"pstack@cursor-plugins":true}}' | Set-Content -Encoding utf8 (Join-Path $full '.claude\settings.json')

Write-Output "fixture home: $full"
