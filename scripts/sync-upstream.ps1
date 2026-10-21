# Keep this fork in sync with storytold/cadcraft.
#
# The vendor-branch model: `main` is upstream/main plus our commits, replayed with rebase.
# Nothing here rewrites upstream; it only replays our small patch series on top.
#
# Usage:
#   pwsh scripts/sync-upstream.ps1            # rebase + verify
#   pwsh scripts/sync-upstream.ps1 -NoCi      # rebase only
param(
    [string]$Upstream = "https://github.com/storytold/cadcraft.git",
    [string]$Branch = "main",
    [switch]$NoCi
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 1. Make sure the upstream remote exists.
$remotes = git remote
if ($remotes -notcontains "upstream") {
    git remote add upstream $Upstream
}

# 2. Fetch, then replay our commits on top of upstream's tip.
git fetch upstream --prune
$before = git rev-parse --short HEAD
git rebase "upstream/main"
$after = git rev-parse --short HEAD
Write-Host "Rebased: $before -> $after"

# 3. Verify before anyone pushes. A failing CI means: do not push.
if (-not $NoCi) {
    cargo xtask ci
}

Write-Host ""
Write-Host "Next:" -ForegroundColor Cyan
Write-Host "  git push --force-with-lease origin $Branch"
