# Launch both nudge-svc (resident tray) and nudge-app (on-demand GUI).
# Usage: .\scripts\launch-both.ps1
#
# nudge-app is ALWAYS rebuilt here via `tauri build`, never via a plain
# `cargo build --release`. A plain cargo build omits Tauri's `custom-protocol`
# feature (normally injected by the `tauri build` CLI), which silently
# produces a binary that points at the Vite dev server (localhost:1420)
# instead of the bundled UI -> "localhost refused to connect" at launch.
# See NEXTSTEPS.md session 37. Do not bypass this rebuild step.
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
$svc = Join-Path $root "target\release\nudge-svc.exe"
$appDir = Join-Path $root "crates\nudge-app"
$app = Join-Path $appDir "src-tauri\target\release\nudge-app.exe"

if (Get-Process nudge-svc -ErrorAction SilentlyContinue) {
    Write-Host "nudge-svc already running."
} else {
    Start-Process -FilePath $svc -WorkingDirectory $root
    Write-Host "Started nudge-svc."
}

Write-Host "Rebuilding nudge-app via 'tauri build' (ensures custom-protocol feature is compiled in)..."
Push-Location $appDir
try {
    npm run tauri build -- --no-bundle
    if ($LASTEXITCODE -ne 0) {
        throw "tauri build failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}

Start-Process -FilePath $app -WorkingDirectory $root
Write-Host "Started nudge-app."
