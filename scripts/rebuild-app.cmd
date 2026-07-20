@echo off
REM rebuild-app.cmd — production rebuild of nudge-app + run new google tests; logs to build.log for readback. One-shot, no runtime cost.
cd /d "%USERPROFILE%\Claude\Projects\nudge-bot\crates\nudge-app"
echo === cargo test -p nudge-app === > "%USERPROFILE%\Claude\Projects\nudge-bot\build.log"
cargo test -p nudge-app --lib google:: >> "%USERPROFILE%\Claude\Projects\nudge-bot\build.log" 2>&1
echo === tauri build === >> "%USERPROFILE%\Claude\Projects\nudge-bot\build.log"
call npm run tauri build -- --no-bundle >> "%USERPROFILE%\Claude\Projects\nudge-bot\build.log" 2>&1
echo === DONE exit %ERRORLEVEL% === >> "%USERPROFILE%\Claude\Projects\nudge-bot\build.log"
