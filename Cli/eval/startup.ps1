# Startup latency gate. Windows process creation (plus antivirus scanning)
# costs ~20-35 ms for *any* executable on a typical machine, so the gate
# measures what cosmostui adds on top of a trivial Rust executable from the
# same release build (zsandbox --help), and prints absolute numbers too.
#
#   --version            <= baseline + 10 ms   (median of -Runs)
#   fixture ping turn    <= baseline + 120 ms  (config, boot, one turn, session file)
#
#   powershell -File eval\startup.ps1 [-Bin <path>] [-Runs 20]
param(
  [string]$Bin = "",
  [int]$Runs = 20,
  [double]$VersionOverMs = 10,
  [double]$PingOverMs = 120
)
$ErrorActionPreference = "Continue"   # native stderr must not abort the run
$root = Split-Path -Parent $PSScriptRoot
$rel = Join-Path $root "rust-tui\target\release"
if (-not $Bin) { $Bin = Join-Path $rel "cosmostui.exe" }
$base = Join-Path $rel "zsandbox.exe"
foreach ($p in @($Bin, $base)) { if (-not (Test-Path $p)) { Write-Host "FAIL  $p not found (cargo build --release --offline -p cosmostui -p zsandbox)"; exit 1 } }

$env:COSMOS_CLI_HOME = Join-Path $env:TEMP "cosmos-startup-gate"
$env:COSMOS_NO_ONBOARDING = "1"
$env:COSMOS_CLI_KEYRING = "0"
Remove-Item -Recurse -Force $env:COSMOS_CLI_HOME -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $env:COSMOS_CLI_HOME | Out-Null

function Median([string]$exe, [string[]]$argv) {
  & $exe @argv *> $null   # warm-up
  $t = @()
  for ($i = 0; $i -lt $Runs; $i++) {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $exe @argv *> $null
    $sw.Stop()
    $t += $sw.Elapsed.TotalMilliseconds
  }
  $s = $t | Sort-Object
  return [math]::Round($s[[int]($s.Count / 2)], 1)
}

$b = Median $base @("--help")
$v = Median $Bin @("--version")
$p = Median $Bin @("-p", "ping", "--model", "fixture", "--output", "json-stream")
Write-Host ("      baseline (zsandbox --help) median {0} ms" -f $b)
$fail = 0
$dv = [math]::Round($v - $b, 1)
$dp = [math]::Round($p - $b, 1)
if ($dv -le $VersionOverMs) { Write-Host "PASS  --version median $v ms (+$dv ms over baseline, limit +$VersionOverMs)" }
else { Write-Host "FAIL  --version median $v ms (+$dv ms over baseline, limit +$VersionOverMs)"; $fail++ }
if ($dp -le $PingOverMs) { Write-Host "PASS  fixture ping median $p ms (+$dp ms over baseline, limit +$PingOverMs)" }
else { Write-Host "FAIL  fixture ping median $p ms (+$dp ms over baseline, limit +$PingOverMs)"; $fail++ }
Remove-Item -Recurse -Force $env:COSMOS_CLI_HOME -ErrorAction SilentlyContinue
exit $fail
