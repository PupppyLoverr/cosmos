# Windows packaging: build the release binaries and produce
#   target/package/zeron-<version>-windows-x86_64.zip   (portable + update payload)
#   target/package/zeron-<version>-windows-x86_64.exe   (raw exe update payload)
#   target/package/manifest.json                        (release/updater manifest)
#   target/package/Cosmos-Setup-x86_64.exe              (when Inno Setup is installed)
#
# The zip carries both entry points (zeron.exe, cosmos.exe — same binary, two
# brandings), zeron-update.json pointing at -ReleasesUrl, licenses, and a
# per-user install.cmd that puts the bin dir on PATH.
#
# Usage: scripts/package-windows.ps1 -ReleasesUrl <https-feed>
# Env:   SKIP_INSTALLER=1 to skip the Inno Setup step even when iscc is present.

param(
    [Parameter(Mandatory)][string]$ReleasesUrl
)
$ErrorActionPreference = 'Stop'
if (-not $ReleasesUrl.StartsWith('https://')) { throw 'Release feed must use HTTPS' }
$root = Split-Path $PSScriptRoot -Parent
Push-Location $root
try {
    cargo build --release --locked -p zeron
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    # Explicit pipes also work for the GUI-subsystem executable in CI. A
    # PowerShell collection match does not populate the scalar $Matches map.
    $probe = [Diagnostics.ProcessStartInfo]::new()
    $probe.FileName = (Resolve-Path -LiteralPath './target/release/zeron.exe').Path
    $probe.Arguments = '--version'
    $probe.UseShellExecute = $false
    $probe.CreateNoWindow = $true
    $probe.RedirectStandardOutput = $true
    $probe.RedirectStandardError = $true
    $process = [Diagnostics.Process]::Start($probe)
    try {
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(10000)) {
            $process.Kill()
            throw 'Executable version probe timed out'
        }
        $versionMatch = [regex]::Match($stdout.Result.Trim(), '\A(?:zeron|cosmos) (\d+\.\d+\.\d+)\z')
        if ($process.ExitCode -ne 0 -or -not $versionMatch.Success) {
            throw "Cannot read executable version: $($stderr.Result)"
        }
        $version = $versionMatch.Groups[1].Value
    } finally { $process.Dispose() }
    $out = Join-Path $root 'target/package'
    $stage = Join-Path $out "zeron-$version-windows-x86_64"
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath './target/release/zeron.exe' -Destination (Join-Path $stage 'zeron.exe')
    Copy-Item -LiteralPath './target/release/cosmos.exe' -Destination (Join-Path $stage 'cosmos.exe')
    @{ releases_url = $ReleasesUrl } | ConvertTo-Json | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $stage 'zeron-update.json')
    Copy-Item -LiteralPath 'LICENSE','THIRD_PARTY_NOTICES.md' -Destination $stage

    # Per-user installer: drops both binaries under %LOCALAPPDATA%\Cosmos\bin
    # and prepends it to the user's PATH. No admin, no shell restart needed for
    # `cosmos`/`zeron` in new terminals.
    @'
@echo off
setlocal
set "DEST=%LOCALAPPDATA%\Cosmos\bin"
if not exist "%DEST%" mkdir "%DEST%"
copy /y "%~dp0zeron.exe" "%DEST%\zeron.exe" >nul
copy /y "%~dp0cosmos.exe" "%DEST%\cosmos.exe" >nul
powershell -NoProfile -Command "$p=[Environment]::GetEnvironmentVariable('Path','User'); if ($p -notlike '*Cosmos\bin*') { [Environment]::SetEnvironmentVariable('Path', $p + ';%DEST%', 'User') }"
echo Installed Cosmos to %DEST% (added to your user PATH - new terminals see it).
'@ | Set-Content -Encoding ASCII (Join-Path $stage 'install.cmd')

    $licenses = Join-Path $stage 'licenses/fonts'
    New-Item -ItemType Directory -Force -Path $licenses | Out-Null
    Copy-Item -Path 'crates/ui/assets/fonts/licenses/*' -Destination $licenses
    Compress-Archive -Path "$stage/*" -DestinationPath "$stage.zip" -Force
    Copy-Item -LiteralPath './target/release/zeron.exe' -Destination "$stage.exe"
    $file = Split-Path "$stage.exe" -Leaf
    $hash = (Get-FileHash -LiteralPath "$stage.exe" -Algorithm SHA256).Hash.ToLowerInvariant()
    @{ version = $version; files = @{ $file = @{ sha256 = $hash } } } |
        ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8NoBOM -LiteralPath (Join-Path $out 'manifest.json')
    Write-Host "Packaged $stage.zip"

    # Optional Inno Setup installer — only when iscc is on PATH (CI installs it
    # via choco). SKIP_INSTALLER=1 forces zip-only packaging.
    $iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($iscc -and $env:SKIP_INSTALLER -ne '1') {
        $iss = Join-Path $out 'cosmos.iss'
        $bin = Join-Path $root 'target/release/zeron.exe'
        $cosmosBin = Join-Path $root 'target/release/cosmos.exe'
        @"
#define Version "$version"
[Setup]
AppName=Cosmos
AppVersion={#Version}
AppPublisher=Cosmos
DefaultDirName={localappdata}\Cosmos
DefaultGroupName=Cosmos
OutputDir=$out
OutputBaseFilename=Cosmos-Setup-x86_64
; Installer .exe icon; the embedded resource in zeron.exe covers shortcuts and
; the Apps & Features entry (UninstallDisplayIcon just points back at it).
SetupIconFile=$root\dist\windows\zeron.ico
UninstallDisplayIcon={app}\bin\cosmos.exe
Compression=lzma2
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "$bin"; DestDir: "{app}\bin"; DestName: "zeron.exe"; Flags: ignoreversion
Source: "$cosmosBin"; DestDir: "{app}\bin"; DestName: "cosmos.exe"; Flags: ignoreversion

[Icons]
Name: "{group}\Cosmos"; Filename: "{app}\bin\cosmos.exe"
Name: "{autodesktop}\Cosmos"; Filename: "{app}\bin\cosmos.exe"

[Registry]
; Per-user PATH entry so `cosmos` resolves in new terminals.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}\bin"; Check: NeedsAddPath

[Code]
function NeedsAddPath(): Boolean;
var P: String;
begin
  RegQueryStringValue(HKCU, 'Environment', 'Path', P);
  Result := Pos('Cosmos\bin', P) = 0;
end;
"@ | Set-Content -Encoding ASCII $iss
        & $iscc.Source $iss | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "iscc failed ($LASTEXITCODE)" }
        Write-Host "wrote $out\Cosmos-Setup-x86_64.exe"
    } else {
        Write-Host "Inno Setup not found (or SKIP_INSTALLER=1) - zip only"
    }
} finally { Pop-Location }
