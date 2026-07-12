# Register nudge-svc to start at user logon (no elevation needed).
# Usage: .\scripts\register_tasks.ps1 [-ExePath <path to nudge-svc.exe>]
param(
    [string]$ExePath = (Join-Path $PSScriptRoot "..\target\release\nudge-svc.exe" | Resolve-Path)
)

$cfg = Join-Path $env:LOCALAPPDATA "nudge-bot"
if (-not (Test-Path (Join-Path $cfg "rules.toml"))) {
    New-Item -ItemType Directory -Force $cfg | Out-Null
    Copy-Item (Join-Path $PSScriptRoot "..\rules.example.toml") (Join-Path $cfg "rules.toml")
    Write-Host "Seeded $cfg\rules.toml from rules.example.toml"
}

$action = New-ScheduledTaskAction -Execute $ExePath
$trigger = New-ScheduledTaskTrigger -AtLogOn -User $env:USERNAME
$settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -ExecutionTimeLimit ([TimeSpan]::Zero)
Register-ScheduledTask -TaskName "nudge-svc" -Action $action -Trigger $trigger -Settings $settings -Force | Out-Null
Write-Host "Registered scheduled task 'nudge-svc' (at logon): $ExePath"

if (-not (Get-Process nudge-svc -ErrorAction SilentlyContinue)) {
    Start-ScheduledTask -TaskName "nudge-svc"
    Write-Host "Started nudge-svc."
} else {
    Write-Host "nudge-svc already running."
}
