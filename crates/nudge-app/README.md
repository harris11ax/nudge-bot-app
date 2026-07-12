# nudge-app

Planner / Triggers / Calendar GUI for nudge-bot (UI-PLAN.md). **On-demand, not
resident** — launched from the tray, a notification click, or the Start menu. The
resident `nudge-svc` still renders all notifications; this app only reads/writes
shared state (the `tasks` table in `%LOCALAPPDATA%\nudge-bot\sessions.db` and
`rules.toml`) and pings the svc to live-reload after a write.

Stack: **Tauri v2** (Rust backend) + **Svelte 5 / Vite** frontend. WebView2 is
system-provided on Win11. Deliberately **kept out of the Cargo workspace**
(root `Cargo.toml` `exclude`) so Tauri's dep tree/lockfile never perturbs the
lean resident `nudge-svc` build.

## Layout
```
package.json / vite.config.js / svelte.config.js / index.html   frontend build
src/                      Svelte UI (App = 3-pane shell; lib/ = tabs + store)
  App.svelte              left tab rail · content router · resizable to-do sidebar
  lib/Planner.svelte      week list (≤7d / missed) + variable list, filters
  lib/Triggers.svelte     quick-add (`text @ time [recur]`) + fallback form
  lib/TaskPage.svelte     task detail
  lib/Sidebar.svelte      to-do list, next deadline pinned
  lib/store.svelte.js     shared task state (runes) over the command layer
  lib/api.js              invoke() wrappers
  lib/format.js           pure display helpers
src-tauri/                Rust backend
  src/lib.rs              Tauri commands (list_tasks/add_quickadd/add_task/delete_task)
  src/db.rs               tasks-table writer/reader (schema mirrors nudge-svc persist.rs)
  tauri.conf.json         window + bundle config
  gen-icons.mjs           regenerates icons/ (solid rounded-blue tile)
```

## Dev / build
```
npm install
npm run tauri dev      # hot-reload dev window (needs WebView2, present on Win11)
npm run tauri build -- --no-bundle   # release exe without a full installer
npm run tauri build    # bundled installer
node src-tauri/gen-icons.mjs   # regenerate placeholder icons
```
Frontend-only checks: `npm run build`. Backend-only: `cd src-tauri && cargo check`.

**Never build the release exe with a plain `cargo build --release`** (even from inside
`src-tauri`) — Tauri only embeds the bundled frontend when its `custom-protocol` feature is
compiled in, which is off by default and normally set by the `tauri build` CLI. A plain cargo
build silently links a binary that still points at the Vite dev server
(`http://localhost:1420`); with no dev server running you get WebView2's "localhost refused to
connect" instead of the app, with no build-time error. Always go through `npm run tauri build`
(see `scripts\launch-both.ps1` at the repo root, which does this automatically before every
launch).

## Status (P1)
Planner + Triggers live and wired to the DB. Calendar / Settings / Log are
placeholders (P2+). Notification click-through and the sidebar **Start** button
are stubbed pending the svc-side launch/focus IPC. Icons are procedural
placeholders — replace via `npm run tauri icon <art.png>` when branding exists.
