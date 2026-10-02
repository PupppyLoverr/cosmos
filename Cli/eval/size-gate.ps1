# Binary size gate: the published cosmostui binary must stay at or under 8 MiB,
# and the UI work (skins, motion, buddy, /customise) may add at most
# -MaxGrowthMiB over the size before it (-BaseMiB).
#   powershell -File eval\size-gate.ps1 [-Bin <path>] [-MaxMiB 8] [-BaseMiB 5.71] [-MaxGrowthMiB 1.0]
# Build first: cargo build --release --offline -p cosmostui  (in rust-tui\)
param(
  [string]$Bin = "",
  [double]$MaxMiB = 8.0,
  [double]$BaseMiB = 5.71,
  [double]$MaxGrowthMiB = 1.0
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $Bin) {
  $exe = if ($IsLinux -or $IsMacOS) { "cosmostui" } else { "cosmostui.exe" }
  $Bin = Join-Path $root "rust-tui\target\release\$exe"
}
if (-not (Test-Path $Bin)) {
  Write-Host "FAIL  $Bin not found - build it with: cargo build --release --offline -p cosmostui"
  exit 1
}
$bytes = (Get-Item $Bin).Length
$mib = [math]::Round($bytes / 1MB, 2)
if ($mib -gt $MaxMiB) {
  Write-Host "FAIL  $Bin is $mib MiB ($bytes bytes) > $MaxMiB MiB - cut a dependency"
  exit 1
}
$growth = [math]::Round($mib - $BaseMiB, 2)
if ($growth -gt $MaxGrowthMiB) {
  Write-Host "FAIL  $Bin grew $growth MiB over the $BaseMiB MiB baseline (> $MaxGrowthMiB MiB)"
  exit 1
}
Write-Host "PASS  $Bin is $mib MiB ($bytes bytes) <= $MaxMiB MiB, +$growth MiB over $BaseMiB (<= $MaxGrowthMiB)"
exit 0
