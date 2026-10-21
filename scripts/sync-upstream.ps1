# Keep this fork in sync with storytold/cadcraft.
#
# The vendor-branch model: `main` is upstream/main plus our commits, replayed with rebase.
# Nothing here rewrites upstream; it only replays our small patch series on top.
#
# Usage:
#   pwsh scripts/sync-upstream.ps1            # rebase + verify
#   pwsh scripts/sync-upstream.ps1 -NoCi      # rebase only
#   pwsh scripts/sync-upstream.ps1 -Push      # rebase + verify + push to the fork
param(
    [string]$Upstream = "https://github.com/storytold/cadcraft.git",
    [string]$Branch = "main",
    [switch]$NoCi,
    [switch]$Push
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# 1. Make sure the upstream remote exists.
$remotes = git remote
if ($remotes -notcontains "upstream") {
    git remote add upstream $Upstream
}

# 2. Nothing half-done: a dirty tree would be rebased with the changes in it.
if (git status --porcelain) {
    Write-Error "The working tree has changes. Commit or stash them before syncing."
}

# 3. Fetch, then replay our commits on top of upstream's tip.
git fetch upstream --prune
$before = git rev-parse --short HEAD
git rebase "upstream/main"
$after = git rev-parse --short HEAD
Write-Host "Rebased: $before -> $after"

# 4. Verify before anyone pushes. A failing CI means: do not push.
if (-not $NoCi) {
    cargo xtask ci
}

if ($Push) {
    # --force-with-lease: the push only lands if the fork is where we think it is.
    git push --force-with-lease origin $Branch
    Write-Host "Pushed to origin/$Branch" -ForegroundColor Green
    Write-Host ""
    Write-Host "Next:" -ForegroundColor Cyan
    Write-Host "  cargo xtask parity   # refresh the menu-breadth checklist"
    exit 0
}

Write-Host ""
Write-Host "Next:" -ForegroundColor Cyan
Write-Host "  git push --force-with-lease origin $Branch"
