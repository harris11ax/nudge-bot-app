# BUG: Google tab shows "No OAuth client configured" on `.vbs` launch — RESOLVED

## RESOLUTION (2026-07-17)
Root cause **confirmed**: stale WebView2 cache. The app's `EBWebView\Default\Cache`
and `Code Cache` were dated Jul 12 06:54 while the exe/dist rebuilt Jul 17 18:18–18:23.
WebView2 served a pre-fix JS chunk (with `not_configured` as the default) from its
Code Cache; the fresh backend (which never returns `NotConfigured` when the client
file reads OK) was correct all along.

Fixes applied:
- Cleared the stale `Cache` + `Code Cache` (one-time remediation).
- **Prevention:** added `--disable-http-cache` to the window's
  `additionalBrowserArgs` in `src-tauri/tauri.conf.json` (kept Tauri's default
  `--disable-features=...` args, since setting this key overrides them). App serves
  only tiny embedded local assets, so disabling the WebView2 HTTP cache has no cost
  and stops any future build from being shadowed by a cached chunk.
  (Note: the sanctioned `app.security.headers` config can't set `Cache-Control` —
  Tauri only allows a fixed allowlist of header names — and `on_web_resource_request`
  would require building the window in code instead of via config, so the browser-arg
  route is the minimal fix.)
- Removed the temp diagnostic in `load_client_config()` and deleted the debug log.
- Rebuilt (`npx tauri build`), exe 19:12.

Everything below is the original investigation, kept for reference.

---

# BUG: Google tab shows "No OAuth client configured" on `.vbs` launch

## Symptom
Launching via `scripts/launch-silent.vbs` (double-click / shortcut), the Google
tab renders the setup-needed message:

> No OAuth client configured. Create a Desktop-app OAuth client ... save to
> `%LOCALAPPDATA%\nudge-bot\google_client.json`

When Claude launched the *same* fresh exe via `wscript.exe` from PowerShell, the
Google tab worked (calendar visible, connected). So it reproduces on the user's
own launch, not on a shell-inherited-env launch.

## What is NOT the cause (ruled out with evidence)
1. **Missing/invalid credentials** — file is valid, 205 bytes, at the right path.
2. **Stale exe** — rebuilt with `npx tauri build`; the `.vbs` launches
   `crates/nudge-app/src-tauri/target/release/nudge-app.exe` (rebuilt 18:23,
   12,610,560 bytes). The diagnostic code below only exists in the new build and
   it *did* run, proving the fresh exe is what launches.
3. **`%LOCALAPPDATA%` / launch-context env difference** — diagnostic log proves
   the backend resolves the correct path and reads the file successfully under
   the failing launch.
4. **Backend returning `NotConfigured`** — it does NOT. See below.
5. **Frontend `load()` throwing / race** — already hardened (`loading` initial
   state + 5x retry on exception in `GoogleConnect.svelte`). `not_configured` is
   a valid non-error return, so retry can't and doesn't apply here.

## KEY EVIDENCE — the contradiction to solve
Temp diagnostic (added to `load_client_config()` in
`crates/nudge-app/src-tauri/src/google/mod.rs`) logs to
`%TEMP%\nudge-google-debug.log`. After the user's *failing* `.vbs` launch:

```
google_status: LOCALAPPDATA=C:\Users\harri\AppData\Local path=C:\Users\harri\AppData\Local\nudge-bot\google_client.json -> read ok, 205 bytes
```

Every logged launch says **read ok**. Therefore `load_client_config()` returns
`Some`, and `google_status()` returns `Connected`/`Disconnected` — **never
`NotConfigured`**.

Yet the UI shows the `not_configured` branch, which in
`crates/nudge-app/src/lib/GoogleConnect.svelte` renders ONLY when
`status === "not_configured"`.

=> The rendered frontend is NOT reflecting the backend response. Backend is
fresh and correct; the **frontend the WebView displays is stale**.

## PRIME HYPOTHESIS — stale WebView2 asset cache
Tauri v2 serves the bundled frontend to WebView2, which persists a cache at:

```
C:\Users\harri\AppData\Local\com.nudgebot.app\EBWebView\Default
```

(App identifier `com.nudgebot.app`; EBWebView user-data dir confirmed present.)

A cached `index.html` / old `index-*.js` chunk from a build *before* the
`not_configured`-default fix is being served, so the UI shows the old default
even though the new backend runs. The shell-launched run may have differed by
timing/first-paint, which is why it appeared to work once.

Note: current bundle chunk is `dist/assets/index-BTlFP6DI.js` and this hash has
been stable across the last two builds — if content changed but the hash did
not, a content-cache keyed on URL would keep serving the old bytes.

## NEXT STEPS for the new session
1. **Confirm the hypothesis:** fully quit the app, delete
   `C:\Users\harri\AppData\Local\com.nudgebot.app\EBWebView` (or just `Default`),
   relaunch via `.vbs`. If the Google tab is now correct, cache was the cause.
2. **If confirmed, fix properly** (don't rely on manual cache-clear):
   - Ensure vite emits content-hashed filenames that actually change when
     content changes (verify why `index-BTlFP6DI.js` hash was stable across
     builds — possible identical content, or a caching config issue).
   - Consider disabling WebView2 caching for the app shell, or send
     `Cache-Control: no-store` on the Tauri asset protocol responses, or bump
     the app version so the WebView2 partition rotates.
3. **If cache is NOT the cause:** instrument the frontend — log the raw value
   returned by `googleStatus()` (in `api.js`) to the console AND to a file, and
   have the user launch via `.vbs`, to see the exact string the frontend
   receives vs. what it renders. Also verify `api.js` `googleStatus()` maps the
   Tauri command result correctly (enum snake_case: `not_configured` /
   `disconnected` / `connected`).

## CLEANUP (must revert before done)
- Remove the TEMP DIAG block in `load_client_config()`
  (`crates/nudge-app/src-tauri/src/google/mod.rs`) that writes
  `nudge-google-debug.log`.
- Delete `%TEMP%\nudge-google-debug.log`.

## Relevant files
- `crates/nudge-app/src/lib/GoogleConnect.svelte` — UI, `not_configured` branch.
- `crates/nudge-app/src/lib/api.js` — `googleStatus()` IPC wrapper.
- `crates/nudge-app/src-tauri/src/google/mod.rs` — `google_status`,
  `load_client_config` (+ temp diagnostic).
- `crates/nudge-app/src-tauri/src/db.rs` — `config_dir()` (LOCALAPPDATA).
- `scripts/launch-silent.vbs` — the failing launch path.
- Build: `cd crates/nudge-app && npx tauri build` (runs `npm run build` first).
  Kill `nudge-app`/`nudge-svc` before building or the exe is locked (os error 5).
