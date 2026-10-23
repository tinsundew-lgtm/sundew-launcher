# Start LimeCraft Launcher (Rust).

# Adds cargo + WinLibs mingw to PATH, then runs `cargo run --release`
# in the project root. Uses only ASCII so the script parses under any
# Windows PowerShell code page.

$ProjectRoot = $PSScriptRoot

# 1) cargo (rustup)
$CargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
if (Test-Path $CargoBin) {
    $env:PATH = "$CargoBin;$env:PATH"
} else {
    Write-Warning "Cargo bin not found: $CargoBin - skipping."
}
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $Extra = @(
        (Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'),
        (Join-Path $env:RUSTUP_HOME 'bin\cargo.exe')
    )
    $found = $Extra | Where-Object { Test-Path $_ -ErrorAction SilentlyContinue } | Select-Object -First 1
    if ($found) {
        $env:PATH = "$(Split-Path $found);$env:PATH"
    }
}

# 2) WinLibs mingw64 (required at link time: the default LLVM-MinGW toolchain
# has no gcc, so lld fails with "unable to find -lgcc_eh"). Pick the WinLibs
# installation that actually contains gcc.exe.
$WinLibs = Get-ChildItem "$env:LOCALAPPDATA\Microsoft\WinGet\Packages\" -Directory -ErrorAction SilentlyContinue |
    Where-Object { $_.Name -like 'BrechtSanders.WinLibs.POSIX.UCRT_*' -and (Test-Path (Join-Path $_.FullName 'mingw64\bin\gcc.exe')) } |
    Select-Object -First 1
if ($WinLibs) {
    $mingwbin = Join-Path $WinLibs.FullName 'mingw64\bin'
    if (Test-Path $mingwbin) {
        $env:PATH = "$mingwbin;$env:PATH"
        Write-Host "WinLibs mingw64 added to PATH: $mingwbin" -ForegroundColor DarkGray
    } else {
        Write-Warning "WinLibs found but no mingw64\bin: $($WinLibs.FullName)"
    }
} else {
    Write-Warning "WinLibs mingw64 not found. If linking fails (missing -lgcc_eh) install WinLibs via winget."
}

# 3) Point TMP/TEMP at a local build-temp folder. Some toolchains' C compiler
# (e.g. gcc used to build the `ring` crypto crate) cannot create temp files in
# the system TEMP in constrained environments ("Cannot create temporary file
# in ...\Temp: Permission denied"). A per-project temp dir avoids that.
$BuildTemp = Join-Path $ProjectRoot '.build-tmp'
New-Item -ItemType Directory -Force -Path $BuildTemp | Out-Null
$env:TMP = $BuildTemp
$env:TEMP = $BuildTemp

Push-Location $ProjectRoot
try {
    Write-Host "LimeCraft: building and starting from $ProjectRoot" -ForegroundColor Green
    cargo run --release
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
