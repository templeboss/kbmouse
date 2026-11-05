# Kill the running kbmouse, rebuild the release exe, start it again.
# kbmouse runs elevated, so this script elevates itself to be able to stop it.

$ErrorActionPreference = 'Stop'

$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).
    IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
if (-not $admin) {
    Start-Process powershell -Verb RunAs -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$PSCommandPath`""
    exit
}

Set-Location $PSScriptRoot
$exe = Join-Path $PSScriptRoot 'target\release\kbmouse.exe'

try {
    Get-Process kbmouse -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $exe } |
        ForEach-Object { Stop-Process -Id $_.Id -Force; $_.WaitForExit(5000) | Out-Null }

    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }

    Start-Process $exe
    Write-Host 'kbmouse rebuilt and restarted.' -ForegroundColor Green
    Start-Sleep -Seconds 2
} catch {
    Write-Host $_ -ForegroundColor Red
    Read-Host 'Press Enter to close'
}
