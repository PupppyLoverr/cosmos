# Cosmos conformance gate — every harness must PASS every check.
# Run from repo root:  powershell -File eval\conformance.ps1
$ErrorActionPreference = "Stop"
$root   = Split-Path -Parent $PSScriptRoot
$cli    = Join-Path $root "rust-tui\target\debug\zcli.exe"
$zdev   = Join-Path $root "zdev\dist\zdev.exe"
$zagy   = Join-Path $root "go-ssh\zagy.exe"
$merid  = Join-Path $root "meridian-bridge\target\debug\meridian.exe"
$node   = "node"
$env:MERIDIAN_BIN = $merid

$checks = @()
function Check($name, $script) {
  # native stderr → ErrorRecord; keep EAP=Continue inside checks so only Assert throws.
  $eap = $ErrorActionPreference; $ErrorActionPreference = "Continue"
  try { & $script | Out-Null; $script:checks += @{ name = $name; ok = $true } ; Write-Host "PASS  $name" -ForegroundColor Green }
  catch { $script:checks += @{ name = $name; ok = $false; err = "$_" } ; Write-Host "FAIL  $name  -- $_" -ForegroundColor Red }
  finally { $ErrorActionPreference = $eap }
}
function Assert($cond, $msg) { if (-not $cond) { throw $msg } }
function RunHarness($h, $argv, [string]$cwd = $null) {
  # native-command stderr becomes an ErrorRecord; under EAP=Stop it would throw.
  $save = $PWD.Path
  $eap = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    if ($cwd) { Set-Location $cwd }
    switch ($h) {
      "zcli" { & $cli @argv 2>$null }
      "zagy" { & $zagy @argv 2>$null }
      "zdev" { & $zdev @argv 2>$null }
    }
  } finally { Set-Location $save; $ErrorActionPreference = $eap }
}
function EventsOf($lines) { @($lines | Where-Object { $_ -match '^\{' } | ConvertFrom-Json) }

# --- binaries exist ---
Check "binaries built" {
  Assert (Test-Path $cli)   "zcli.exe missing"
  Assert (Test-Path $zagy)  "zagy.exe missing"
  Assert (Test-Path $merid) "meridian.exe missing"
  Assert (Test-Path $zdev)  "zdev.exe missing"
}

# --- C1: tools dump byte-identical across all three + canonical ---
Check "tools dump byte-identical" {
  $canon = Join-Path $root "contracts\tools.json"
  $tmp = Join-Path $env:TEMP "cosmos-conf"; New-Item -ItemType Directory -Force $tmp | Out-Null
  cmd /c "`"$cli`" tools dump > `"$tmp\r.dump`"" 2>$null
  cmd /c "`"$zdev`" tools dump > `"$tmp\py.dump`"" 2>$null
  cmd /c "`"$zagy`" tools dump > `"$tmp\go.dump`"" 2>$null
  $c = [IO.File]::ReadAllBytes($canon)
  foreach ($f in "r.dump","py.dump","go.dump") {
    $p = Join-Path $tmp $f
    Assert (Test-Path $p) "$f not produced"
    $b = [IO.File]::ReadAllBytes($p)
    Assert ($b.Length -eq $c.Length) "$f length $($b.Length) vs canonical $($c.Length)"
    for ($i=0; $i -lt $b.Length; $i++) { Assert ($b[$i] -eq $c[$i]) "$f byte $i differs" }
  }
}

# --- C2: fixture ping → pong, json-stream pure, events valid ---
foreach ($h in @("zcli","zdev","zagy")) {
  Check "$h fixture ping/json-stream" {
    $out = @(RunHarness $h @("-p","ping","--model","fixture","--output","json-stream"))
    Assert ($out.Count -gt 0) "$h produced no events"
    $out | & $node (Join-Path $root "eval\validate-events.mjs") | Out-Null
    Assert ($LASTEXITCODE -eq 0) "$h events failed schema"
    $evs = EventsOf $out
    $txt = @(($evs | Where-Object { $_.kind -eq "assistant" }).data.text) -join ""
    Assert ($txt -match "pong") "$h assistant text was '$txt'"
    Assert (@($evs | Where-Object { $_.kind -eq "usage" }).Count -ge 1) "$h emitted no usage event"
    $sess = @($evs | Where-Object { $_.kind -eq "session_start" })[0]
    Assert ($sess.data.boot_tokens -gt 0) "$h boot_tokens=0"
    Assert (@($evs | Where-Object { $_.kind -eq "session_end" }).Count -ge 1) "$h no session_end"
  }
}

# --- C3: session ops (in an isolated home) ---
Check "session list/export/branch/tree" {
  $tmp = Join-Path $env:TEMP "cosmos-sessops"
  if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
  $homeDir = Join-Path $tmp "home"; New-Item -ItemType Directory -Force $homeDir | Out-Null
  $env:COSMOS_CLI_HOME = $homeDir
  RunHarness "zcli" @("-p","ping","--model","fixture","--output","json") | Out-Null
  $list = RunHarness "zcli" @("session","list")
  Assert ($list -match "01") "session list empty"
  $id = ($list | Select-String -Pattern '01[0-9a-z]{24}').Matches[0].Value
  $exp = @(RunHarness "zcli" @("session","export",$id,"--format","jsonl"))
  $exp | & $node (Join-Path $root "eval\validate-events.mjs") | Out-Null
  Assert ($LASTEXITCODE -eq 0) "exported events invalid"
  $md = @(RunHarness "zcli" @("session","export",$id))
  Assert ($md[0] -match "^#") "default export not markdown"
  $tree = @(RunHarness "zcli" @("session","tree",$id))
  Assert ($tree.Count -ge 1) "tree empty"
  $evs = @($exp | ConvertFrom-Json)
  $at = ($evs | Where-Object { $_.kind -eq "user" } | Select-Object -First 1).id
  Assert ($at) "no user event to branch at"
  RunHarness "zcli" @("session","branch",$id,"--at",$at) | Out-Null
  Assert ($LASTEXITCODE -eq 0) "branch failed"
  Remove-Item Env:\COSMOS_CLI_HOME
}

# --- C4: cross-harness resume (zcli → zdev → zagy all append to one session) ---
Check "cross-harness session resume" {
  $tmp = Join-Path $env:TEMP "cosmos-xharness"
  if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
  $homeDir = Join-Path $tmp "home"; New-Item -ItemType Directory -Force $homeDir | Out-Null
  $env:COSMOS_CLI_HOME = $homeDir
  RunHarness "zcli" @("-p","ping","--model","fixture","--output","json") | Out-Null
  $list = RunHarness "zcli" @("session","list")
  $id = ($list | Select-String -Pattern '01[0-9a-z]{24}').Matches[0].Value
  $file = Join-Path $homeDir "sessions\$id.jsonl"
  $n1 = (Get-Content $file).Count
  RunHarness "zdev" @("-p","ping","--model","fixture","--resume",$id) | Out-Null
  $n2 = (Get-Content $file).Count
  Assert ($n2 -gt $n1) "zdev did not append ($n1 -> $n2)"
  RunHarness "zagy" @("-p","ping","--model","fixture","--resume",$id) | Out-Null
  $n3 = (Get-Content $file).Count
  Assert ($n3 -gt $n2) "zagy did not append ($n2 -> $n3)"
  Get-Content $file | & $node (Join-Path $root "eval\validate-events.mjs") | Out-Null
  Assert ($LASTEXITCODE -eq 0) "mixed-harness session failed schema"
  $harnesses = Get-Content $file | ConvertFrom-Json | ForEach-Object { $_.harness } | Sort-Object -Unique
  Assert ($harnesses.Count -eq 3) "expected 3 harnesses, got $($harnesses -join ',')"
  Remove-Item Env:\COSMOS_CLI_HOME
}

# --- C5: memory roundtrip through meridian bridge ---
Check "memory add/search via bridge" {
  $env:COSMOS_CLI_HOME = Join-Path $env:TEMP "cosmos-memhome"
  New-Item -ItemType Directory -Force $env:COSMOS_CLI_HOME | Out-Null
  RunHarness "zcli" @("memory","add","cosmos-conformance-marker-7f3e") | Out-Null
  $res = RunHarness "zcli" @("memory","search","conformance-marker")
  Assert ($res -match "conformance-marker") "zcli search missed: $res"
  $res2 = RunHarness "zagy" @("memory","search","conformance-marker")
  Assert ($res2 -match "conformance-marker") "zagy search missed: $res2"
  Remove-Item Env:\COSMOS_CLI_HOME
}

# --- C6: sandbox probes ---
Check "sandbox probes (4)" {
  Push-Location (Join-Path $root "rust-tui")
  $t = (cargo test -p zsandbox -- --test-threads=1 2>&1 | Out-String)
  Pop-Location
  Assert ($t -match "4 passed") "sandbox tests: $($t -split "`n" | Select-String 'test result' | Select-Object -Last 1)"
}

# --- C7: permission + workspace boundaries ---
Check "plan-mode deny + path escape deny" {
  $tmp = Join-Path $env:TEMP "cosmos-deny"
  if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
  New-Item -ItemType Directory -Force $tmp | Out-Null
  Set-Content (Join-Path $tmp "v.txt") "keep"
  $o1 = RunHarness "zcli" @("-p",'tool:write {\"path\":\"v.txt\",\"content\":\"x\"}',"--mode","plan","--cwd",$tmp,"--output","json-stream","--model","fixture")
  $r1 = @(EventsOf $o1 | Where-Object { $_.kind -eq "tool_result" })[0]
  Assert ($r1.data.content -match "denied|plan") "plan-mode write not denied: $($r1.data.content)"
  $o2 = RunHarness "zcli" @("-p",'tool:read {\"path\":\"../secret.txt\"}',"--allow","read","--cwd",$tmp,"--output","json-stream","--model","fixture")
  $r2 = @(EventsOf $o2 | Where-Object { $_.kind -eq "tool_result" })[0]
  Assert ($r2.data.content -match "escapes workspace") "escape not denied: $($r2.data.content)"
}

# --- C8b: ACP stdio handshake on every harness ---
# Fresh home → no user config → fixture default (user config may point at
# a live provider that isn't up).
$env:COSMOS_CLI_HOME = Join-Path $env:TEMP "cosmos-acphome"
New-Item -ItemType Directory -Force $env:COSMOS_CLI_HOME | Out-Null
foreach ($h in @("zcli","zdev","zagy")) {
  Check "$h acp handshake" {
    $bin = if ($h -eq "zcli") { $cli } elseif ($h -eq "zagy") { $zagy } else { $zdev }
    $o = & $node (Join-Path $root "eval\acp-smoke.mjs") $bin 2>$null
    Assert ($LASTEXITCODE -eq 0) "$h acp failed: $o"
    Assert ($o -match "OK acp") "unexpected: $o"
  }
}
Remove-Item Env:\COSMOS_CLI_HOME

# --- C10: task worktree (contract §10) ---
Check "task worktree create+cleanup" {
  $tmp = Join-Path $env:TEMP "cosmos-wt"
  if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
  New-Item -ItemType Directory -Force $tmp | Out-Null
  Push-Location $tmp
  git init -q; git config user.email t@t; git config user.name t
  git commit -qm init --allow-empty
  $env:COSMOS_CLI_HOME = Join-Path $tmp "home"; New-Item -ItemType Directory -Force $env:COSMOS_CLI_HOME | Out-Null
  $o = RunHarness "zcli" @("-p",'tool:task {\"prompt\":\"ping\",\"worktree\":true}',"--allow","task,read","--cwd",$tmp,"--output","json-stream","--model","fixture")
  $r = @(EventsOf $o | Where-Object { $_.kind -eq "tool_result" })[0]
  Assert ($r.data.content -match "worktree branch: cosmos-task-") "no branch in result: $($r.data.content)"
  $wt = git worktree list --porcelain | Select-String "worktree"
  Assert (@($wt).Count -eq 1) "worktree not removed: $wt"
  Assert (-not (git branch | Select-String "cosmos-task")) "branch not removed"
  Pop-Location
  Remove-Item Env:\COSMOS_CLI_HOME
}

# --- C9: tool call terminates (fixture regression) ---
Check "tool call terminates in 1 turn" {
  $tmp = Join-Path $env:TEMP "cosmos-t1"
  if (Test-Path $tmp) { Remove-Item -Recurse -Force $tmp }
  New-Item -ItemType Directory -Force $tmp | Out-Null
  Set-Content (Join-Path $tmp "a.txt") "hello"
  $o = RunHarness "zcli" @("-p",'tool:read {\"path\":\"a.txt\"}',"--allow","read","--cwd",$tmp,"--output","json-stream","--model","fixture")
  $calls = @(EventsOf $o | Where-Object { $_.kind -eq "tool_result" }).Count
  Assert ($calls -eq 1) "expected 1 tool_result, got $calls"
}

Write-Host ""
$pass = @($checks | Where-Object ok).Count; $tot = $checks.Count
Write-Host "conformance: $pass/$tot PASS"
if ($pass -ne $tot) { $checks | Where-Object { -not $_.ok } | ForEach-Object { Write-Host "  FAIL: $($_.name) -- $($_.err)" }; exit 1 }
exit 0
