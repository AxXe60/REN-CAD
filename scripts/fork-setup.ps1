# Turn this checkout into a fork with an `origin` remote, and push.
#
# Usage:
#   pwsh scripts/fork-setup.ps1 -Owner <github-user-or-org> [-Name cadcraft]
param(
    [Parameter(Mandatory = $true)][string]$Owner,
    [string]$Name = "cadcraft",
    [string]$Upstream = "https://github.com/storytold/cadcraft.git"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

if ((git remote) -notcontains "upstream") {
    git remote add upstream $Upstream
}
if ((git remote) -contains "origin") {
    git remote set-url origin "https://github.com/$Owner/$Name.git"
} else {
    git remote add origin "https://github.com/$Owner/$Name.git"
}

Write-Host "Create the empty repository on GitHub first:" -ForegroundColor Cyan
Write-Host "  https://github.com/new  (name: $Name, no README, no .gitignore)"
Read-Host "Press Enter when the repository exists"

git push -u origin main
git branch --set-upstream-to=upstream/main main | Out-Null

Write-Host ""
Write-Host "Done. Weekly sync: pwsh scripts/sync-upstream.ps1" -ForegroundColor Green
