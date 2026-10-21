# Build environment for this fork on Windows.
#
# Why: a full workspace build needs ~10 GB. This machine's C: drive is at 100%, so the target
# directory lives on D:. Run this once per shell:
#
#   . .\scripts\env.ps1
#
# Then `cargo xtask ci`, `cargo build -p cadcraft`, and the tests all use D:.

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$targetRoot = "D:\cargo-target"
if (-not (Test-Path $targetRoot)) {
    New-Item -ItemType Directory -Path $targetRoot | Out-Null
}
$env:CARGO_TARGET_DIR = Join-Path $targetRoot "LASDraft"

# Full debug info and incremental caches roughly triple the size of target/ and buy nothing for
# CI on this machine (the C: drive is full and D: is shared with the LiDAR archive).
$env:CARGO_PROFILE_DEV_DEBUG = "0"
$env:CARGO_INCREMENTAL = "0"

Write-Host "CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR" -ForegroundColor Green
$free = (Get-PSDrive -Name D).Free / 1GB
Write-Host ("D: free: {0:N1} GB" -f $free)
