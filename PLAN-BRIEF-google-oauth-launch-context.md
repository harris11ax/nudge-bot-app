# Planning Brief: Google tab shows "No OAuth client configured" — ONLY on the user's own launch

> Handoff brief for Claude Cowork to build an implementation/investigation plan.
> Prior bug notes live in `BUG-google-oauth-notconfigured.md` (earlier theories, now
> mostly disproven — read this brief first, treat that file as history).

## 1. Problem statement
Launching nudge-app via `scripts/launch-silent.vbs` (double-click / Explorer, run
by `wscript.exe`), the **Google** tab renders the setup-needed message:

> No OAuth client configured. Create a Desktop-app OAuth client … save to
> `%LOCALAPPDATA%\nudge-bot\google_client.json`

When **Claude** launches the *same* exe (via `wscript`/direct exec from a
PowerShell/Bash shell), the Google tab works (Connected, calendar visible).
The failure reproduces **only on the user's own launch path**, and it survived a
full reboot. The user's request: *"rebuild how the app is launched — something is
not working when you launch the app versus when I launch the app."*

## 2. Success criteria
1. Google tab shows the correct connected/disconnected state when the user
   launches via their normal method (double-click a shortcut / `.vbs`).
2. The fix does not depend on Claude pre-launching, clearing caches, or running
   anything from a shell first.
3. Root cause is *identified with evidence* (not patched blind), and the
   launch mechanism is made robust so it can't regress.

## 3. Confirmed facts (verified live, 2026-07-17, current session)
| Fact | Evidence |
|---|---|
| The failing UI is served by the **freshly rebuilt** exe | Running PID's `ExecutablePath` = `…\crates\nudge-app\src-tauri\target\release\nudge-app.exe`, `LastWriteTime` 07/17 19:13:17 (the current build). Not a stale/installed copy. |
| **No** second/installed copy of nudge-app.exe exists | No `nudge-app.exe` in Program Files, `AppData\Local\Programs`, or elsewhere reachable; only the target/release build + the NSIS installer artifact. |
| `google_client.json` is **valid & parseable** | `%LOCALAPPDATA%\nudge-bot\google_client.json` = 145 bytes, flat form: `{"client_id":"96546835144-…apps.googleusercontent.com","client_secret":"GOCSPX-…"}`. Parses via `parse_client_config` flat branch. |
| Backend logic can only return `NotConfigured` if `load_client_config()` → `None`, i.e. the file **read** or **parse** fails **in the app process** | `google/mod.rs::google_status` (lines ~133–142). |
| Frontend renders the `not_configured` message **only** on exact string `"not_configured"` | `GoogleConnect.svelte` line 110; `api.js::googleStatus` maps the raw enum string. Enum serializes snake_case (`#[serde(rename_all="snake_case")]`). |

**The core contradiction:** file is valid + exe is fresh + cache disabled, yet the
user's launch shows `not_configured`. That is only possible if, *in the user's
launched process*, `load_client_config()` returns `None` — meaning that process
resolves a **different path** or a **different/empty `%LOCALAPPDATA%`**, or reads a
different file, than a shell-launched process does.

`config_dir()` (`db.rs:18`) = `std::env::var("LOCALAPPDATA") + "\nudge-bot"`. It
**panics** if `LOCALAPPDATA` is unset — so an *unset* var would crash, not show
not_configured. But a *different* value (redirected profile, different user
context, roamed vs local) would silently point at a folder with no client file.

## 4. Theories DISPROVEN (do not re-pursue without new evidence)
1. **Stale WebView2 asset/code cache** — cleared `EBWebView\Default\Cache` +
   `Code Cache`, added `--disable-http-cache` browser arg, rebuilt, user rebooted.
   Still fails. Disproven.
2. **Stale exe** — running exe is the 19:13 build. Disproven.
3. **Invalid/missing credentials file** — file is valid, parseable, present. Disproven.
4. **Frontend race / `load()` throwing** — a throw leaves `status="loading"`
   ("Checking Google connection…"), not the not_configured message. Disproven as
   the cause of *this specific* message.

## 5. Live hypotheses to test (ranked)
**H1 — `%LOCALAPPDATA%` (or env) differs under Explorer→wscript vs shell→wscript.**
Most likely. Explorer-spawned processes inherit the interactive shell's
environment block; a shell-spawned wscript inherits the (possibly richer/different)
shell env. If the user's Explorer session has `LOCALAPPDATA` pointing somewhere
without the client file (folder redirection, a per-machine vs per-user profile
quirk, an env override in their user profile), the app reads nothing → `None` →
NotConfigured. **Prior "read ok, 205 bytes" logs were likely Claude's launches,
not the user's — the assumption that they proved the user's path was wrong.**

**H2 — Working-directory / relative-path assumption.** `.vbs` sets no CWD; wscript's
default CWD may differ. `config_dir()` uses an env var, not CWD, so this only bites
if some other path resolution (svc handshake, token path) is relative. Lower
likelihood but cheap to rule out.

**H3 — A stale/old `nudge-svc.exe` interferes.** The `.vbs` also starts
`root\target\release\nudge-svc.exe`, which is dated **07-14** (old, 2.7 MB) while
the app is 07-17. If any shared-state / signaling / DB-path contract drifted
between the 07-14 svc and 07-17 app, behavior could diverge by launch order. The
svc doesn't serve the Google UI, so this is unlikely to *directly* cause
not_configured, but the version skew should be eliminated and ruled out.

**H4 — Timing / IPC init under wscript.** The app may query `google_status` before
some init completes only under the wscript-spawned parent. Would more likely show
"loading" than not_configured; low priority.

## 6. Required diagnostic (the decisive experiment)
Add a **launch-context probe** that writes, on every `google_status()` call, a
single line to `%TEMP%\nudge-google-debug.log` capturing ALL of:
- a **launch nonce / timestamp** and the process's own **PID + parent process name**
  (so a log line is unambiguously tied to *which* launch — this was the gap before),
- `LOCALAPPDATA` **raw value**, the **resolved** `google_client.json` path,
- read outcome + **byte count**, parse outcome (Some/None),
- the **final `ConnectState`** the function returns.

Then run the **two-launch diff**:
1. User double-clicks the `.vbs` (or their shortcut) — the *failing* path.
2. Claude launches the same exe from a shell — the *working* path.
Compare the two log lines. The differing field (almost certainly `LOCALAPPDATA` /
resolved path / byte count) is the root cause.

> Note: the app is a `windows_subsystem="windows"` GUI with no console, so
> file-based logging (not stdout) is required. Reuse the temp-log pattern that
> already existed in `load_client_config`.

## 7. Once root cause is known — likely fixes to plan for
- **If H1 (env/path):** stop trusting the ambient `LOCALAPPDATA`. Options:
  (a) resolve the config dir via the Win32 **known-folder API**
  (`SHGetKnownFolderPath(FOLDERID_LocalAppData)`) instead of the env var, which is
  immune to a mangled env block; (b) have the `.vbs` explicitly set a stable env
  before `sh.Run`; (c) fall back to a second known location if the primary is
  missing. Prefer (a) — it's the real robustness fix and matches the user's
  "rebuild how the app is launched" ask.
- **If H3 (svc skew):** rebuild `nudge-svc` from current source so app+svc are the
  same generation, and make the `.vbs` launch the correct svc path
  (`root\target\release` vs the app's own target tree — verify which is intended).
- Make the launch reproducible: single canonical launcher, correct absolute paths,
  optional single-instance guard so a half-initialized process can't shadow a good one.

## 8. Key files
- `scripts/launch-silent.vbs` — the failing launcher. Starts `nudge-svc.exe`
  (`root\target\release`) then `nudge-app.exe` (`root\crates\nudge-app\src-tauri\target\release`), both via `WScript.Shell.Run`, async, inheriting the caller's env.
- `crates/nudge-app/src-tauri/src/db.rs:18` — `config_dir()` (reads `LOCALAPPDATA`, **panics if unset**).
- `crates/nudge-app/src-tauri/src/google/mod.rs` — `google_status` (~133), `load_client_config` (~30), `parse_client_config` (~63), `ConnectState` enum (~124).
- `crates/nudge-app/src/lib/GoogleConnect.svelte:110` — the `not_configured` branch.
- `crates/nudge-app/src/lib/api.js:22` — `googleStatus()` IPC wrapper.
- `crates/nudge-app/src-tauri/tauri.conf.json` — window config; now carries
  `additionalBrowserArgs: "--disable-features=… --disable-http-cache"` (from the
  disproven cache fix — harmless, decide whether to keep).

## 9. Constraints / gotchas
- **Build:** `cd crates/nudge-app && npx tauri build` (runs `npm run build` first).
  Full build ≈ 3–4 min. **Kill `nudge-app.exe` and `nudge-svc.exe` first** or the
  exe is file-locked (`os error 5`). Build also emits MSI + NSIS bundles.
- Windows-only (DPAPI token sealing); GUI has no console → **must log to a file**.
- The observation gap that misled the last session: log lines were not tied to a
  specific launch. **Any new diagnostic must self-identify its launch** (nonce +
  parent PID) so working vs failing launches can't be confused again.
- Current process state at brief time: app PID + svc running from the user's launch;
  kill before rebuilding.

## 10. Suggested plan shape for Cowork
1. Instrument `google_status`/`load_client_config` with the launch-context probe (§6).
2. Rebuild; have the user run the **two-launch diff**; collect both log lines.
3. Confirm the differing field → pin the hypothesis.
4. Implement the matching fix from §7 (favor known-folder path resolution).
5. Rebuild both app **and** svc from current source; fix `.vbs` paths; add a
   single-instance guard if warranted.
6. User verifies via their normal double-click launch, from a cold boot.
7. Remove the temp diagnostic; delete `%TEMP%\nudge-google-debug.log`; reconcile
   `BUG-google-oauth-notconfigured.md`.
