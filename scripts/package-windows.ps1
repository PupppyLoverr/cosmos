# Windows packaging: build the Cosmos release binary and produce
#   target/package/Cosmos-Setup-x86_64.exe              (per-user Inno installer)
#   target/package/cosmos-<version>-windows-x86_64.zip  (portable + update payload)
#   target/package/manifest.json                        (release/updater manifest)
#
# The installer and zip ship cosmos.exe only.
#
# Usage: scripts/package-windows.ps1 [-ReleasesUrl <https-feed>]
# Env:   SKIP_INSTALLER=1 to skip the Inno Setup step even when iscc is present.

param(
    [string]$ReleasesUrl = 'https://github.com/PupppyLoverr/cosmos/releases/latest/download'
)
$ErrorActionPreference = 'Stop'
if (-not $ReleasesUrl.StartsWith('https://')) { throw 'Release feed must use HTTPS' }
$root = Split-Path $PSScriptRoot -Parent

function Get-WindowsPackageArch([string]$Path) {
    # Match zeron-update's `std::env::consts::ARCH` so the standalone .exe
    # name agrees with crates/update/src/windows.rs::artifact. Read the built
    # executable's PE machine type rather than this PowerShell process's
    # architecture: x64 PowerShell under ARM64 emulation reports X64 no matter
    # which toolchain rustc used.
    $stream = [IO.File]::OpenRead($Path)
    try {
        $reader = [IO.BinaryReader]::new($stream)
        $stream.Position = 0x3C
        $stream.Position = $reader.ReadUInt32()
        if ($reader.ReadUInt32() -ne 0x4550) { throw "Not a PE executable: $Path" }
        $machine = $reader.ReadUInt16()
    } finally { $stream.Dispose() }
    switch ($machine) {
        0x8664 { 'x86_64' }
        0xAA64 { 'aarch64' }
        default { throw ('Unsupported Windows executable machine type: 0x{0:X4}' -f $machine) }
    }
}

Push-Location $root
try {
    cargo build --release --locked -p zeron --bin cosmos
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed' }
    # Explicit pipes also work for the GUI-subsystem executable in CI. A
    # PowerShell collection match does not populate the scalar $Matches map.
    $probe = [Diagnostics.ProcessStartInfo]::new()
    $probe.FileName = (Resolve-Path -LiteralPath './target/release/cosmos.exe').Path
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
    $arch = Get-WindowsPackageArch $probe.FileName
    $stage = Join-Path $out "cosmos-$version-windows-$arch"
    New-Item -ItemType Directory -Force -Path $stage | Out-Null
    Copy-Item -LiteralPath './target/release/cosmos.exe' -Destination (Join-Path $stage 'cosmos.exe')
    # BOM-free UTF-8: serde_json rejects a BOM, and -Encoding utf8NoBOM only
    # exists on pwsh 7 — on Windows PowerShell 5.1 it isn't even a valid enum.
    $utf8NoBom = [System.Text.UTF8Encoding]::new($false)
    [IO.File]::WriteAllText((Join-Path $stage 'zeron-update.json'), (@{ releases_url = $ReleasesUrl } | ConvertTo-Json) + "`n", $utf8NoBom)
    Copy-Item -LiteralPath 'LICENSE','THIRD_PARTY_NOTICES.md' -Destination $stage

    # Per-user installer: drops cosmos.exe under %LOCALAPPDATA%\Cosmos\bin
    # and prepends it to the user's PATH.
    @'
@echo off
setlocal
set "DEST=%LOCALAPPDATA%\Cosmos\bin"
if not exist "%DEST%" mkdir "%DEST%"
copy /y "%~dp0cosmos.exe" "%DEST%\cosmos.exe" >nul
powershell -NoProfile -Command "$p=[Environment]::GetEnvironmentVariable('Path','User'); if ($p -notlike '*Cosmos\bin*') { [Environment]::SetEnvironmentVariable('Path', $p + ';%DEST%', 'User') }"
echo Installed Cosmos to %DEST% (added to your user PATH - new terminals see it).
'@ | Set-Content -Encoding ASCII (Join-Path $stage 'install.cmd')

    $licenses = Join-Path $stage 'licenses/fonts'
    New-Item -ItemType Directory -Force -Path $licenses | Out-Null
    Copy-Item -Path 'crates/ui/assets/fonts/licenses/*' -Destination $licenses
    Compress-Archive -Path "$stage/*" -DestinationPath "$stage.zip" -Force
    $file = Split-Path "$stage.zip" -Leaf
    $hash = (Get-FileHash -LiteralPath "$stage.zip" -Algorithm SHA256).Hash.ToLowerInvariant()
    [IO.File]::WriteAllText((Join-Path $out 'manifest.json'), (@{ version = $version; files = @{ $file = @{ sha256 = $hash } } } | ConvertTo-Json -Depth 4) + "`n", $utf8NoBom)
    Write-Host "Packaged $stage.zip"

    # Optional Inno Setup installer — only when iscc is on PATH (CI installs it
    # via choco). SKIP_INSTALLER=1 forces zip-only packaging.
    $iscc = (Get-Command iscc.exe -ErrorAction SilentlyContinue).Source
    if (-not $iscc -and (Test-Path 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe')) {
        $iscc = 'C:\Program Files (x86)\Inno Setup 6\ISCC.exe'
    }
    if (-not $iscc -and (Test-Path 'C:\Program Files\Inno Setup 6\ISCC.exe')) {
        $iscc = 'C:\Program Files\Inno Setup 6\ISCC.exe'
    }
    if ($iscc -and $env:SKIP_INSTALLER -ne '1') {
        $iss = Join-Path $out 'cosmos.iss'
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
SetupIconFile=$root\dist\windows\zeron.ico
UninstallDisplayIcon={app}\bin\cosmos.exe
Compression=lzma2/ultra64
SolidCompression=yes
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "$cosmosBin"; DestDir: "{app}\bin"; DestName: "cosmos.exe"; Flags: ignoreversion
Source: "$stage\zeron-update.json"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "$root\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "$root\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\Cosmos"; Filename: "{app}\bin\cosmos.exe"
Name: "{autodesktop}\Cosmos"; Filename: "{app}\bin\cosmos.exe"

[Run]
Filename: "{app}\bin\cosmos.exe"; Description: "Launch Cosmos"; Flags: nowait postinstall skipifsilent

[Registry]
; Per-user PATH entry so ``cosmos`` resolves in new terminals.
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
        & $iscc $iss | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "iscc failed ($LASTEXITCODE)" }
        Write-Host "wrote $out\Cosmos-Setup-x86_64.exe"
    } else {
        Write-Host "Inno Setup not found (or SKIP_INSTALLER=1) - zip only"
    }
} finally { Pop-Location }
