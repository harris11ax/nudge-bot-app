# Stop nudge-svc and remove the logon task. Config/db in %LOCALAPPDATA%\nudge-bot is kept;
# pass -PurgeData to delete it too.
param([switch]$PurgeData)

Get-Process nudge-svc -ErrorAction SilentlyContinue | Stop-Process -Force -Confirm:$false
Unregister-ScheduledTask -TaskName "nudge-svc" -Confirm:$false -ErrorAction SilentlyContinue
Write-Host "Removed scheduled task 'nudge-svc' and stopped the process."

if ($PurgeData) {
    Remove-Item -Recurse -Force (Join-Path $env:LOCALAPPDATA "nudge-bot") -ErrorAction SilentlyContinue
    Write-Host "Purged %LOCALAPPDATA%\nudge-bot."
}
