@echo off
rem Launch LimeCraft via PowerShell bypassing execution policy.
rem This works when double-clicking (the .ps1 alone is often blocked by policy).
setlocal
cd /d "%~dp0"
set "PS=%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe"
if not exist "%PS%" set "PS=powershell.exe"
"%PS%" -NoProfile -ExecutionPolicy Bypass -File "%~dp0start.ps1"
pause
