# Windows packaging: build the release binary and produce
#   target/package/zeron-<version>-windows-<arch>.zip
#   target/package/Zeron-Setup-<arch>.exe   (when Inno Setup is installed)
#
# The zip is the portable package AND the self-update payload — crates/update
# resolves it via headless_artifact() and unpacks it with System32 bsdtar.
#
# Usage: scripts/package-windows.ps1
# Env:   PROFILE=debug for a fast unoptimized package (CI smoke); default release.
#        SKIP_INSTALLER=1 to skip the Inno Setup step even when iscc is present.

$ErrorActionPreference = 'Stop'

$Root    = Split-Path -Parent $PSScriptRoot
$Profile = if ($env:PROFILE) { $env:PROFILE } else { 'release' }
$Arch    = $env:PROCESSOR_ARCHITECTURE.ToLower()          # amd64 / arm64
if ($Arch -eq 'amd64') { $Arch = 'x86_64' }
$Version = (Select-String -Path "$Root\Cargo.toml" -Pattern '^version\s*=\s*"(.+)"' |
            Select-Object -First 1).Matches[0].Groups[1].Value
$OutDir  = "$Root\target\package"
$Stage   = "$OutDir\zeron-$Version-windows-$Arch"
$Zip     = "$Stage.zip"

Push-Location $Root
try {
    if ($Profile -eq 'release') { cargo build --release -p zeron }
    else                        { cargo build -p zeron }
    $Bin = "$Root\target\$Profile\zeron.exe"
    if (-not (Test-Path $Bin)) { throw "build did not produce $Bin" }

    Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $Stage, $Zip
    New-Item -ItemType Directory -Force $Stage | Out-Null
    Copy-Item $Bin "$Stage\zeron.exe"
    New-Item -ItemType Directory -Force "$Stage\licenses\fonts" | Out-Null
    Copy-Item "$Root\crates\ui\assets\fonts\licenses\*" "$Stage\licenses\fonts\"

    # Per-user installer: drops zeron.exe under %LOCALAPPDATA%\Zeron\bin and
    # prepends it to the user's PATH. No admin, no shell restart needed for
    # `zeron` in new terminals (PATH change applies to new processes).
    @'
@echo off
setlocal
set "DEST=%LOCALAPPDATA%\Zeron\bin"
if not exist "%DEST%" mkdir "%DEST%"
copy /y "%~dp0zeron.exe" "%DEST%\zeron.exe" >nul
powershell -NoProfile -Command "$p=[Environment]::GetEnvironmentVariable('Path','User'); if ($p -notlike '*Zeron\bin*') { [Environment]::SetEnvironmentVariable('Path', $p + ';%DEST%', 'User') }"
echo Installed zeron to %DEST% (added to your user PATH - new terminals see it).
'@ | Set-Content -Encoding ASCII "$Stage\install.cmd"

    Compress-Archive -Path "$Stage\*" -DestinationPath $Zip -Force
    Write-Host "wrote $Zip"

    # Optional Inno Setup installer — only when iscc is on PATH (CI installs it
    # via choco). SKIP_INSTALLER=1 forces zip-only packaging.
    $iscc = Get-Command iscc.exe -ErrorAction SilentlyContinue
    if ($iscc -and $env:SKIP_INSTALLER -ne '1') {
        $iss = "$OutDir\zeron.iss"
        @"
#define Version "$Version"
[Setup]
AppName=Zeron
AppVersion={#Version}
AppPublisher=Zeron
DefaultDirName={localappdata}\Zeron
DefaultGroupName=Zeron
OutputDir=$OutDir
OutputBaseFilename=Zeron-Setup-$Arch
; Installer .exe icon; the embedded resource in zeron.exe covers shortcuts and
; the Apps & Features entry (UninstallDisplayIcon just points back at it).
SetupIconFile=$Root\dist\windows\zeron.ico
UninstallDisplayIcon={app}\bin\zeron.exe
Compression=lzma2
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible

[Files]
Source: "$Bin"; DestDir: "{app}\bin"; Flags: ignoreversion

[Icons]
Name: "{group}\Zeron"; Filename: "{app}\bin\zeron.exe"
Name: "{autodesktop}\Zeron"; Filename: "{app}\bin\zeron.exe"

[Registry]
; Per-user PATH entry so `zeron` resolves in new terminals.
Root: HKCU; Subkey: "Environment"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}\bin"; Check: NeedsAddPath

[Code]
function NeedsAddPath(): Boolean;
var P: String;
begin
  RegQueryStringValue(HKCU, 'Environment', 'Path', P);
  Result := Pos('Zeron\bin', P) = 0;
end;
"@ | Set-Content -Encoding ASCII $iss
        & $iscc.Source $iss | Out-Host
        if ($LASTEXITCODE -ne 0) { throw "iscc failed ($LASTEXITCODE)" }
        Write-Host "wrote $OutDir\Zeron-Setup-$Arch.exe"
    } else {
        Write-Host "Inno Setup not found (or SKIP_INSTALLER=1) - zip only"
    }
} finally {
    Pop-Location
}
