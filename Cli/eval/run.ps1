# Cosmos eval runner — runs eval\tasks\* against each harness.
#   powershell -File eval\run.ps1 [-Harness cosmostui|zdev|zagy|all]
# Deterministic: uses the fixture provider via `tool:` prompts.
param([string]$Harness = "all")
$ErrorActionPreference = "Stop"
$root  = Split-Path -Parent $PSScriptRoot
$cli   = Join-Path $root "rust-tui\target\debug\cosmostui.exe"
$zagy  = Join-Path $root "go-ssh\zagy.exe"
$zdev  = Join-Path $root "zdev\dist\zdev.exe"
$env:MERIDIAN_BIN = Join-Path $root "meridian-bridge\target\debug\meridian.exe"
$results = @()

function RunHarness($h, $prompt, $ws) {
  $save = $PWD.Path; $eap = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try {
    $argv = @("-p", $prompt, "--model", "fixture", "--allow", "read,edit,write,ls,glob,grep,bash",
              "--cwd", $ws, "--output", "json-stream")
    switch ($h) {
      "cosmostui" { & $cli @argv 2>$null }
      "zagy" { & $zagy @argv 2>$null }
      "zdev" { & $zdev @argv 2>$null }
    }
  } finally { Set-Location $save; $ErrorActionPreference = $eap }
}
function LastToolResult($lines) {
  @(($lines | Where-Object { $_ -match '^\{' } | ConvertFrom-Json) |
    Where-Object { $_.kind -eq "tool_result" } | Select-Object -Last 1)
}

foreach ($taskDir in Get-ChildItem (Join-Path $root "eval\tasks") -Directory) {
  $task = $taskDir.Name
  foreach ($h in @("cosmostui","zdev","zagy")) {
    if ($Harness -ne "all" -and $Harness -ne $h) { continue }
    $ws = Join-Path $env:TEMP ("eval-" + $task + "-" + $h)
    if (Test-Path $ws) { Remove-Item -Recurse -Force $ws }
    Copy-Item -Recurse (Join-Path $taskDir.FullName "workspace") $ws
    $env:COSMOS_CLI_HOME = Join-Path $ws ".home"; New-Item -ItemType Directory -Force $env:COSMOS_CLI_HOME | Out-Null
    $ok = $true; $why = ""

    if ($task -eq "t01-read-edit-write") {
      # 1. read the seeded file (proves hashline read path)
      $o = RunHarness $h 'tool:read {\"path\":\"src/app.py\"}' $ws
      if ($LASTEXITCODE -ne 0) { $ok=$false; $why="read exit $LASTEXITCODE" }
      $r = LastToolResult $o
      if ($ok -and $r.data.content -notmatch "file_sha256=") { $ok=$false; $why="read missing sha header" }
      # harvest sha + line anchor from the read result (real contract flow)
      $sha = $null; $anchor = $null
      if ($ok -and $r.data.content -match 'file_sha256=([0-9a-f]{64})') { $sha = $Matches[1] }
      if ($ok -and $r.data.content -match '(?m)^(\d+:[0-9a-f]{6})\|') { $anchor = $Matches[1] }
      if ($ok -and (-not $sha -or -not $anchor)) { $ok=$false; $why="could not parse sha/anchor from read" }
      # 2. write a new file
      $o = RunHarness $h 'tool:write {\"path\":\"notes.md\",\"content\":\"done\"}' $ws
      if ($ok -and -not (Test-Path (Join-Path $ws "notes.md"))) { $ok=$false; $why="notes.md missing" }
      # 3. edit app.py v1→v2 using the harvested file_sha256 + anchor
      if ($ok) {
        $editJson = ('{"path":"src/app.py","file_sha256":"' + $sha + '","edits":[{"start":"' + $anchor + '","end":"' + $anchor + '","new_text":"print(hello-v2)"}]}') -replace '"','\"'
        $o = RunHarness $h "tool:edit $editJson" $ws
        $r = LastToolResult $o
        if ($r.data.is_error -or ((Get-Content (Join-Path $ws "src\app.py") -Raw) -notmatch "v2")) {
          $ok=$false; $why="edit failed: $($r.data.content)"
        }
      }
      # 4. grep finds v2
      $o = RunHarness $h 'tool:grep {\"pattern\":\"v2\"}' $ws
      $r = LastToolResult $o
      if ($ok -and $r.data.content -notmatch "v2") { $ok=$false; $why="grep missed v2" }
      # 5. every emitted stream is schema-valid
      $eapv = $ErrorActionPreference; $ErrorActionPreference = "Continue"
      $vout = Get-ChildItem (Join-Path $env:COSMOS_CLI_HOME "sessions") -Filter *.jsonl |
        ForEach-Object { Get-Content $_.FullName } |
        & node (Join-Path $root "eval\validate-events.mjs") 2>&1
      $vcode = $LASTEXITCODE; $ErrorActionPreference = $eapv
      if ($ok -and $vcode -ne 0) { $ok=$false; $why="session events invalid: $(($vout | Select-Object -First 3) -join ' | ')" }
    }

    if ($task -eq "t02-compact-resume") {
      $o = RunHarness $h "ship the login form" $ws
      if ($LASTEXITCODE -ne 0) { $ok=$false; $why="seed exit $LASTEXITCODE" }
      $list = switch ($h) { "cosmostui" { & $cli session list 2>$null } "zagy" { & $zagy session list 2>$null } default { & $zdev session list 2>$null } }
      $id = ($list | Select-String -Pattern '01[0-9a-z]{24}').Matches[0].Value
      $argv = @("-p","/compact","--model","fixture","--resume",$id,"--output","json-stream")
      $ErrorActionPreference = "Continue"
      switch ($h) { "cosmostui" { & $cli @argv 2>$null } "zagy" { & $zagy @argv 2>$null } default { & $zdev @argv 2>$null } }
      $ErrorActionPreference = "Stop"
      $world = Join-Path $env:COSMOS_CLI_HOME "world\$id.md"
      if ($ok -and -not (Test-Path $world)) { $ok=$false; $why="world.md missing" }
      if ($ok -and ((Get-Content $world -Raw) -notmatch "ship the login form")) { $ok=$false; $why="goal lost after compact" }
    }

    if ($task -eq "t03-repair") {
      $o = RunHarness $h 'tool:Read {\"path\":\"a.txt\"}' $ws
      if ($LASTEXITCODE -ne 0) { $ok=$false; $why="repair exit $LASTEXITCODE" }
      $evs = @($o | Where-Object { $_ -match '^\{' } | ConvertFrom-Json)
      if ($ok -and -not ($evs | Where-Object { $_.kind -eq "tool_repair" })) { $ok=$false; $why="no tool_repair event" }
      $r = LastToolResult $o
      if ($ok -and $r.data.content -notmatch "file_sha256=") { $ok=$false; $why="read after repair failed: $($r.data.content)" }
    }

    $results += @{ task=$task; harness=$h; ok=$ok; why=$why }
    Write-Host ("{0}  {1}/{2} {3}" -f $(if ($ok) {"PASS"} else {"FAIL"}), $task, $h, $why)
    Remove-Item Env:\COSMOS_CLI_HOME -ErrorAction SilentlyContinue
  }
}
$pass = @($results | Where-Object ok).Count
Write-Host "eval: $pass/$($results.Count) PASS"
if ($pass -ne $results.Count) { exit 1 }
