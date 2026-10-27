# Builds the LimeCraft installer for Windows: dist\LimeCraft-<version>-setup-x64.exe
#
#   powershell -ExecutionPolicy Bypass -File packaging\windows\build-installer.ps1
#   ... -SkipBuild    use the exe already in target\release (no cargo build)
#
# Needs Inno Setup 6.4+ (winget install JRSoftware.InnoSetup) and, unless
# -SkipBuild is given, the same Rust toolchain as start.ps1.
# Uses only ASCII so the script parses under any Windows PowerShell code page.
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'

$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$Iss = Join-Path $PSScriptRoot 'limecraft.iss'
$Exe = Join-Path $Root 'target\release\limecraft-launcher.exe'

$Version = (Select-String -Path (Join-Path $Root 'Cargo.toml') -Pattern '^version\s*=\s*"(.+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
Write-Host "LimeCraft ${Version}: building the installer" -ForegroundColor Green

# 1) Inno Setup compiler
$Iscc = (Get-Command ISCC.exe -ErrorAction SilentlyContinue).Source
if (-not $Iscc) {
    $Iscc = @(
        (Join-Path $env:LOCALAPPDATA 'Programs\Inno Setup 6\ISCC.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 6\ISCC.exe')
    ) | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
}
if (-not $Iscc) {
    throw "Inno Setup not found. Install it: winget install JRSoftware.InnoSetup"
}

# 2) the program itself
if (-not $SkipBuild) {
    $CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
    if (Test-Path $CargoBin) { $env:PATH = "$CargoBin;$env:PATH" }
    # WinLibs gcc is needed at link time (see start.ps1)
    $WinLibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\" -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like 'BrechtSanders.WinLibs.POSIX.UCRT_*' -and (Test-Path (Join-Path $_.FullName 'mingw64\bin\gcc.exe')) } |
        Select-Object -First 1
    if ($WinLibs) { $env:PATH = "$(Join-Path $WinLibs.FullName 'mingw64\bin');$env:PATH" }
    $BuildTemp = Join-Path $Root '.build-tmp'
    New-Item -ItemType Directory -Force -Path $BuildTemp | Out-Null
    $env:TMP = $BuildTemp
    $env:TEMP = $BuildTemp

    Push-Location $Root
    try {
        cargo build --release
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally {
        Pop-Location
    }
}
if (-not (Test-Path $Exe)) { throw "Not found: $Exe (build the program first)" }

# 3) the installer
& $Iscc /Q "/DAppVersion=$Version" "/DSourceExe=$Exe" $Iss
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$Setup = Join-Path $Root "dist\LimeCraft-$Version-setup-x64.exe"
Write-Host "Done: $Setup" -ForegroundColor Green
