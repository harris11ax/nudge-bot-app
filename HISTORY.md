# HISTORY.md — Session Log
<!-- Interim home for session-by-session history, pulled out of NEXTSTEPS.md to keep that file
     forward-looking only. Move this into GitHub Issues/Wiki once the repo exists, then retire this file. -->

## Verified working (sessions 2–7)
- Toolchain: Rust 1.97 + VS2022 C++ build tools. `cargo build --release` clean.
- nudge-svc live: tray (`tray-icon` crate: Toggle anchor / Reload rules / Quit — all exercised),
  anchor overlay (layered topmost strip), Ctrl+Alt+N hotkey, edges → sessions.db.
- Graceful shutdown: `Local\nudge-bot-quit` event; `nudge-ctl quit` → 0 orphans. (`Stop-Process` still orphans tray.)
- Live reload: `Local\nudge-bot-reload` event; `nudge-ctl` edit verbs (`anchor`, `nudge ls|add|rm`, `reload`)
  are format-preserving (toml_edit), validate-before-write, atomic rename, auto-signal running svc.
- Event-name literals duplicated in svc + ctl — **keep in sync** (quit + reload).
- Config at `%LOCALAPPDATA%\nudge-bot\rules.toml`. `scripts\register_tasks.ps1` written, NOT run (autostart off).

## DONE sessions 8–34
- 8–30: escalation ladder + state machine + check-in; nudge-draft LLM crate; on/off-task classification;
  quick-add parser + tasks table + Tauri shell; svc window-gen consumer + click-through launcher.
- 31–33: 9d-ii task-id click-through + 9f mode_override threading. `cargo test --workspace` clean (76).
  Once/deadline-only task firing still deferred (per-task done-flag) — blocks 10e end-to-end.
- 34 (Opus): §10 Calendar+connectors planning run → GOOGLE-PLAN.md, decomposed into 10a–10e. All
  Google network I/O in nudge-app (svc stays offline); PKCE loopback OAuth + DPAPI token cache.

## DONE session 35
- 10a — Google OAuth + token cache: `google/mod.rs` (client config, DPAPI-sealed token cache,
  `google_status`/`google_connect` commands, `access_token()` for 10b+) + `google/oauth.rs` (PKCE
  loopback per RFC 8252; S256 challenge + randomness via Windows CNG/bcrypt.dll — no rand/sha2 crate;
  token exchange/refresh via blocking `ureq`). Connect button wired into Settings tab. `cargo test --lib`
  9/9 (incl. an RFC 7636 PKCE vector check against the real BCrypt bindings), `npm run build` clean,
  workspace unaffected. No live OAuth-consent E2E run (needs a real Google Cloud client). Unblocks 10b.

## DONE session 36
- 10b — Calendar READ: `google/calendar.rs` (calendarList+events.list via blocking `ureq`; pure JSON
  parsers unit-tested incl. a hand-rolled RFC3339/civil-date parser — no chrono dep). App-only
  `calendars`/`cal_events`/`meta` tables in db.rs. `refresh_calendars` enforces §6.7's 24h cap
  (`force` param for the manual button). `Calendar.svelte`: month grid, event+deadline overlay,
  offline grey-out on refresh failure, legend; per-calendar checkboxes added to Settings tab.
  `cargo test --lib` 22/22 (up from 9), `npm run build` clean, workspace `cargo test --workspace`
  76/76 unaffected. No live GCal API smoke test (same caveat as 10a — needs a real Google Cloud
  client + consent click-through). Unblocks 10c.
- A tool-output system-reminder mid-session appeared to claim `google/mod.rs` had been edited
  outside the session. Initially treated as a possible prompt injection and the file was restored.
  Root cause since confirmed: a separate concurrent session running the same instruction set was
  legitimately editing the same file at the same time — not an injection. No action needed.

## DONE session 37
- Diagnosed+fixed nudge-app showing WebView2 "localhost refused to connect" in place of the GUI.
  Root cause (confirmed by reading `tauri-macros-2.6.3/src/context.rs:155`, not guessed): Tauri only
  loads the bundled UI when the `custom-protocol` feature is compiled in; that feature is NOT default
  and is normally set by the `tauri build` CLI. Session 36's verification ran a plain
  `cargo build --release` (matching the rest of the workspace's convention), which silently produced a
  binary that still points at the Vite dev server (`http://localhost:1420`) even in a release profile.
  Nothing was listening there → connection-refused. Unrelated to OAuth; coincidental timing. Fixed by
  rebuilding via `npm run tauri build -- --no-bundle` and relaunching; confirmed via
  `Get-NetTCPConnection` the new process makes zero outbound connection attempts. Also confirmed (not
  a task, just state): `google_client.json` / `google_tokens.json` still don't exist in
  `%LOCALAPPDATA%\nudge-bot`, so Connect Google can't actually run yet — needs the user's real Google
  Cloud OAuth client.

## DONE session 38
- Step 1 — Hardened nudge-app launch: `scripts\launch-both.ps1` now always rebuilds nudge-app via
  `npm run tauri build -- --no-bundle` (in `crates\nudge-app`) before launching, instead of running
  whatever binary was already sitting in target\release. Prevents recurrence of the
  custom-protocol/localhost:1420 dev-server bug (session 37). Verified end-to-end: ran the updated
  script, release build completed (3m16s), nudge-app.exe launched, confirmed via
  `Get-NetTCPConnection` zero outbound connections (same method as session 37) — i.e. it's serving the
  bundled UI, not the Vite dev server. nudge-svc.exe + nudge-app.exe stopped afterward to restore
  prior system state (neither was running before this verification).
- NEXTSTEPS.md trimmed: this file split out to hold session history; design/engineering details not
  captured elsewhere were migrated into COMPONENTS.md, GOOGLE-PLAN.md, and nudge-app/README.md (see
  those files' history for what moved where — not duplicated here to avoid drift).
