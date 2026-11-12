param([switch]$SkipLauncherBuild)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$WinLibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages" -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like 'BrechtSanders.WinLibs.POSIX.UCRT_*' -and (Test-Path (Join-Path $_.FullName 'mingw64\bin\gcc.exe')) } | Select-Object -First 1
if ($WinLibs) { $env:PATH = "$(Join-Path $WinLibs.FullName 'mingw64\bin');$env:PATH" }
$env:TEMP = Join-Path $Root '.build-tmp'
$env:TMP = $env:TEMP
New-Item -ItemType Directory -Force $env:TEMP | Out-Null
Push-Location $Root
try {
    if (-not $SkipLauncherBuild) {
        cargo build --release --bin limecraft-launcher
        if ($LASTEXITCODE -ne 0) { throw 'Launcher build failed' }
    }
    $Payload = Join-Path $Root 'target\release\limecraft-launcher.exe'
    if (-not (Test-Path $Payload)) { throw "Missing launcher: $Payload" }
    $env:LIMECRAFT_INSTALLER_PAYLOAD = $Payload
    # Reuse launcher dependency artifacts instead of duplicating the full GUI stack.
    cargo build --release --manifest-path (Join-Path $PSScriptRoot 'Cargo.toml') --target-dir (Join-Path $Root 'target')
    if ($LASTEXITCODE -ne 0) { throw 'Native installer build failed' }
    $Out = Join-Path $Root 'dist\native-installer'
    New-Item -ItemType Directory -Force $Out | Out-Null
    $Destination = Join-Path $Out 'LimeCraft-Setup.exe'
    Copy-Item (Join-Path $Root 'target\release\limecraft-setup.exe') $Destination -Force
    Get-FileHash $Destination -Algorithm SHA256 | Format-List
    Write-Host "Native installer: $Destination"
} finally { Pop-Location }
