# fix-shortcut.ps1 — one-shot: repoints the nudge-bot shortcut(s) at the silent VBS launcher (no console window, no rebuild); run once. No runtime cost.
$ErrorActionPreference = 'Stop'

$root    = Join-Path $env:USERPROFILE 'Claude\Projects\nudge-bot'
$wscript = "$env:WINDIR\System32\wscript.exe"
$vbs     = Join-Path $root 'scripts\launch-silent.vbs'
$icon    = Join-Path $root 'crates\nudge-app\src-tauri\target\release\nudge-app.exe'

foreach ($p in @($wscript, $vbs, $icon)) {
    if (-not (Test-Path -LiteralPath $p)) { throw "Missing: $p" }
}

# Rewrite both the in-project shortcut and the copy in Projects\.
$targets = @(
    (Join-Path $root 'nudge-bot.lnk'),
    (Join-Path $env:USERPROFILE 'Claude\Projects\nudge-bot.lnk')
)

$w = New-Object -ComObject WScript.Shell
foreach ($lnk in $targets) {
    $s = $w.CreateShortcut($lnk)
    $s.TargetPath       = $wscript
    $s.Arguments        = "`"$vbs`""
    $s.WorkingDirectory = $root
    $s.IconLocation     = "$icon,0"
    $s.Description       = 'Launch nudge-svc (tray) + nudge-app (GUI) — silent'
    $s.WindowStyle       = 1
    $s.Save()
    Write-Host "Fixed: $lnk"
}
