# Build environment for this fork on Windows.
#
# Why: a full workspace build needs ~10 GB. The sources live on the H: NVMe drive, and so does
# the target directory. Run this once per shell:
#
#   . .\scripts\env.ps1
#
# Then `cargo xtask ci`, `cargo build -p cadcraft`, and the tests all use H:.

$env:Path = "$env:USERPROFILE\.cargo\bin;$env:Path"

$targetRoot = "H:\cargo-target"
if (-not (Test-Path $targetRoot)) {
    New-Item -ItemType Directory -Path $targetRoot | Out-Null
}
$env:CARGO_TARGET_DIR = Join-Path $targetRoot "RENCAD"

# Full debug info and incremental caches roughly triple the size of target/ and buy nothing for
# CI on this machine.
$env:CARGO_PROFILE_DEV_DEBUG = "0"
$env:CARGO_INCREMENTAL = "0"

Write-Host "CARGO_TARGET_DIR = $env:CARGO_TARGET_DIR" -ForegroundColor Green
$free = (Get-PSDrive -Name H).Free / 1GB
Write-Host ("H: free: {0:N1} GB" -f $free)
