<!-- Boundary: implementation plan for the Google-OAuth launch-context bug (step 9). Scope: app+svc path/env robustness + launcher. No polling, no network added; diagnostic is file-log only, removed at end. Budget: unchanged RSS/wake-ups. -->

# PLAN — Google tab `not_configured` only on the user's own launch

Source brief: [PLAN-BRIEF-google-oauth-launch-context.md](PLAN-BRIEF-google-oauth-launch-context.md).
History/disproven theories: [BUG-google-oauth-notconfigured.md](BUG-google-oauth-notconfigured.md).
Model: **Opus** (root-cause diagnosis + launch-mechanism redesign). One phase per session.

## Problem / Success / Constraints
- **Problem:** user's double-click/`.vbs` launch renders `not_configured`; a shell-launched exe (Claude) shows Connected. Same fresh exe, same valid `google_client.json`. So the *user's* process resolves a different `LOCALAPPDATA`/path and `load_client_config()` → `None`.
- **Success:** correct Google state on the user's normal launch, from cold boot, with no Claude pre-launch/cache-clear/shell step; root cause proven by evidence; launch made regression-proof.
- **Load-bearing constraints:** Windows-only, GUI has **no console → file logging only**; every diagnostic line must **self-identify its launch** (nonce + PID + parent name); no polling/network/messaging added; kill `nudge-app.exe` + `nudge-svc.exe` before any build (`os error 5` lock); build = `cd crates/nudge-app && npx tauri build` (~3–4 min).

## Phase 1 — Instrument (diagnostic probe) · this session ✅ DONE (2026-07-17)
Goal: make the two-launch diff decisive. No behavior change yet.
Result: probe module `launch_probe.rs` added; `google_status` + startup logging wired; app built + verified. Shell launch line: `parent=powershell.exe localappdata=Ok(C:\Users\harri\AppData\Local) path=…\nudge-bot\google_client.json read=ok bytes=145 parse=some state=connected`. Awaiting Phase 2 user double-click/.vbs line to diff.
1. Add `fn probe_log(stage: &str, fields: &str)` (temp module in `google/mod.rs`, or `crates/nudge-app/src-tauri/src/launch_probe.rs`) that appends one line to `%TEMP%\nudge-google-debug.log`. Guard all writes with `let _ =` (never panic, never leak the handle — open/write/drop in one call via `OpenOptions::append`).
2. On **every** `google_status()` call, log ALL of:
   - launch **nonce** (random u32 generated once at process start, stored in a `OnceLock`) + **timestamp**,
   - **PID** (`std::process::id()`) + **parent process name** (WMI/`CreateToolhelp32Snapshot`, or minimal `sysinfo` — cheapest that avoids a heavy dep),
   - `LOCALAPPDATA` **raw** value (`env::var` result, incl. Err),
   - **resolved** `client_config_path()` absolute string,
   - read outcome + **byte count**, parse outcome (Some/None),
   - final `ConnectState` returned.
3. Also log the same nonce line once at app startup (`main`/`setup`) so parentage is captured even if `google_status` is delayed.
4. Do **not** change `config_dir()` yet — Phase 1 only observes.
5. Build app only (svc untouched). Verify the file appears on a shell launch.

**Gate to Phase 2:** brief's H1 requires proof, not assumption. Do not code the fix until the diff is collected.

## Phase 2 — Two-launch diff + root-cause pin · user-in-the-loop, same or next session
1. Kill running app/svc. Have the **user** double-click their `.vbs`/shortcut (failing path) → open Google tab.
2. Claude launches the same exe from a shell (working path) → open Google tab.
3. Diff the two `nudge-google-debug.log` lines. Expected differing field: `LOCALAPPDATA` raw / resolved path / byte count (H1). If identical env but divergent read → re-rank to H2/H4.
4. Record the decisive field in this file before writing any fix.

## Phase 2 RESULT (2026-07-17) — root cause pinned
Decisive diff (`%TEMP%\nudge-google-debug.log`):
- shell/Claude launch: `localappdata=Ok(C:\Users\harri\AppData\Local) path=…\nudge-bot\google_client.json read=ok bytes=145 parse=some state=connected`
- user `.vbs`/`wscript` launch (reproduced twice): **same** env + **same** path string, but `read=ok bytes=205 parse=none state=not_configured`.

**H1/H2 disproven** — env + resolved path identical. Decisive differing field = **file content (bytes/parse)**, not launch context.

Cause: **MSIX filesystem redirection.** Claude Code runs inside the `Claude_pzs8sxrjxfjjc` app container (proven: filesystem-MCP allowed dir is under `…\Packages\Claude_pzs8sxrjxfjjc\LocalCache\…`). Container-launched processes (Claude's shell launch of nudge-app, all of Claude's file tools) have `%LOCALAPPDATA%` transparently redirected to `…\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Local\nudge-bot\`, which holds a **good 145-byte** copy (written 07-12). The user's native `wscript` double-click reads the **true** `%LOCALAPPDATA%\nudge-bot\google_client.json`, which is **205 bytes and unparseable**. Same env-var string in both; two different physical files. Every "shell works" observation was the container copy — Claude's tools cannot see the real file.

Consequence: **Phase 3 (known-folder) and Phase 4 (launcher hardening) are NOT the fix** — path resolution is correct. Real fix = correct the user's real native `google_client.json` content (user runs it in a native shell; Claude's shell is redirected and can't reach the real path). Optional product improvement: make `google_status` distinguish "present but unparseable" from "absent" so a malformed file isn't silently reported as `not_configured`.

## Phase 3 — Fix (branch on the pinned cause) — SUPERSEDED (see Phase 2 result)
- **If H1 (env/path — most likely):** stop trusting ambient `LOCALAPPDATA`.
  - Primary: resolve config dir via Win32 **known-folder API** `SHGetKnownFolderPath(FOLDERID_LocalAppData)` (`windows`/`windows-sys` crate, already an indirect dep via tauri) — immune to a mangled env block. Wrap in `config_dir()`; keep env var only as a logged fallback.
  - Apply the **same** resolution in `nudge-svc`'s `dirs_config()` so app+svc still agree on the shared dir (they share `sessions.db`/tokens — must not diverge).
  - Remove the `.expect("LOCALAPPDATA")` panic path; on total failure return a real error, not a crash.
- **If H2 (CWD/relative):** find the relative path resolution and make it absolute; no `config_dir` change.
- **If H3 (svc skew, 07-14 vs 07-17):** rebuild `nudge-svc` from current source; correct the `.vbs` svc path — confirm intended tree (`root\target\release` vs app's own `src-tauri\target\release`). Eliminate version skew regardless of whether it was causal.

## Phase 4 — Harden the launcher
1. Rebuild **both** app and svc from current source (same generation).
2. Fix `scripts/launch-silent.vbs`: correct absolute paths for both binaries; optionally set a stable env before `sh.Run` as belt-and-suspenders (secondary to the known-folder fix).
3. Add a **single-instance guard** (named mutex/`CreateMutexW`) so a half-initialized process can't shadow a good one.
4. Decide whether to keep `additionalBrowserArgs: --disable-http-cache` in `tauri.conf.json` (from the disproven cache theory — harmless; recommend removing to reduce noise once fix confirmed).

## RESOLUTION (2026-07-17)
Root cause was **user file content**, not launch context: the real native `google_client.json` held pasted Rust source (`pub struct ClientConfig { … }`, 205 bytes) instead of JSON → `parse=none` → `not_configured`. User rewrote it with valid JSON (145 bytes) via a native shell; `.vbs` launch then connected. Phases 3–4 (known-folder / launcher hardening) unneeded.
Done: Phase-1 probe fully removed (`launch_probe.rs` deleted, `mod`/calls/`Win32_System_Diagnostics_ToolHelp` feature reverted), `%TEMP%\nudge-google-debug.log` deleted. Product fix: added `ConnectState::Misconfigured` (file present but unparseable/unreadable) + a distinct frontend message so a malformed file no longer masquerades as "missing". App rebuilt.

## Phase 5 — Verify + clean up (probe removal + fix DONE above)
1. **User** verifies via their normal double-click, from a **cold boot** — Google tab shows correct state without any shell/Claude step. (`verification-before-completion`: evidence before claiming done.)
2. Remove the Phase-1 probe code; delete `%TEMP%\nudge-google-debug.log`.
3. Reconcile `BUG-google-oauth-notconfigured.md` → history; update `HISTORY.md` via `nextsteps-classifier`; check off the step in `NEXTSTEPS.md`.
4. Commit per repo convention (`9: known-folder config dir + launcher hardening`) — **write only; do not push unless asked**.

## Key files
- `scripts/launch-silent.vbs` — launcher (svc `root\target\release`, app `…\src-tauri\target\release`).
- `crates/nudge-app/src-tauri/src/db.rs:18` — `config_dir()` (env var, panics if unset).
- `crates/nudge-app/src-tauri/src/google/mod.rs` — `google_status` (~133), `load_client_config` (~30), `client_config_path` (~26).
- `crates/nudge-app/src/lib/GoogleConnect.svelte:110`; `crates/nudge-app/src/lib/api.js:22`.
- `nudge-svc` `dirs_config()` — must mirror any `config_dir()` change.
- `crates/nudge-app/src-tauri/tauri.conf.json` — browser args / window config.

## Rollback
Probe is additive + guarded; Phase 3 change is one function (`config_dir`) + its svc mirror. If the known-folder path regresses, revert to env-var `config_dir` (keep the panic removed).
