# UI-PLAN.md — nudge-bot Notification + Planner/Calendar GUI Plan
<!-- Boundary: design plan only; resident core (nudge-svc) keeps <30 MB / zero-polling / no-internet budget. GUI is a separate on-demand process with its own budget. -->

## 0. Architectural Split (load-bearing)

```
nudge-svc.exe      resident notifier (existing). Renders ALL notifications.
                   Never hosts the GUI. Keeps zero-polling/no-internet budget.
nudge-app.exe      NEW: planner/settings/calendar GUI. Launched on demand
                   (tray menu, notification click, Start menu). Own process,
                   own budget (may use network for Google APIs). Not resident.
nudge-draft.exe    future LLM companion (unchanged).
ActivityWatch      on/off-task signal source, read at edges (unchanged).
```

Notification click → svc launches `nudge-app.exe --task <id>` (or focuses running instance via named event + WM_COPYDATA) → app opens the task page. IPC stays: named events + SQLite/rules.toml as shared state; app writes tasks, svc reloads on signal (existing reload event).

## 1. Notification Modes (science-grounded)

Mode is decided **at the edge**, per existing design: query ActivityWatch — if recent foreground app matches the active task's app list (or a global "productive apps" list) → ON-TASK mode; else OFF-TASK.

| | OFF-TASK (attention capture) | ON-TASK (minimal interruption) |
|---|---|---|
| Rationale | When off-task, interruption cost is low and salience helps re-engagement; abrupt onset + motion + sound are the strongest exogenous attention cues (attentional capture literature). Encouraging framing beats shaming (approach motivation sustains behavior better than avoidance). | Interruptions during focused work carry high resumption cost (Iqbal & Horvitz: minutes to resume; errors scale with interruption salience). Peripheral, low-salience "ambient" cues inform without forcing a shift; defer full alerts to natural task boundaries where possible. |
| Visual | Centered mid-size panel, high contrast, brief slide-in motion (motion once, then static — no looping animation), encouraging copy ("Ready to start X? It's time."). Escalation ladder L0→L2 as designed. | Slim edge strip (existing anchor), muted colors, no motion, appears in periphery; text like "Y is due at 3:00 — switch when you reach a stopping point." No escalation while on-task unless deadline-critical flag set. |
| Audio | Single distinct sound at L2 (no loops). | Silent, or optional single soft tick. |
| Timing | Immediate at trigger edge. | Fire at edge, but a deadline-critical trigger may re-cue once at T−N min before the hard deadline. |
| Dismissal | [Start] [Snooze] [Skip] one click. | Auto-fades after M min (logged as unacknowledged) or one-click ack. Click anywhere on it → open task page. |

Both modes: never steal focus, never block input, everything logged to sessions.db.

## 2. Main Window Layout (nudge-app)

```
┌──────┬──────────────────────────────┬───────────────┐
│ LEFT │        CONTENT AREA          │ RIGHT SIDEBAR │
│ menu │  (current tab)               │ (to-do, fixed │
│ tabs │                              │  presence,    │
│ ▸/◂  │                              │  resizable)   │
└──────┴──────────────────────────────┴───────────────┘
```

- **Left sidebar:** collapsible (icon-only when collapsed). Tabs: Planner, Calendar, Triggers, Settings, Log.
- **Right sidebar:** always visible, user-resizable width (persisted). Content = to-do list; **next deadline pinned at top** (name, countdown, one-click Start). Below: compact task list sorted by deadline.
- **Content area:** renders active tab.

### Planner tab
- **Week list:** tasks with deadline ≤ 7 days OR missed deadline (missed styled distinctly, sorted first).
- **Variable list:** all other tasks, filters: time range (2 weeks / 1 month / custom), keyword search over title+description, checkbox task-type filters. Filter state persisted.
- Task page (opened by notification click or list click): title, description, deadline, type, trigger(s), history from sessions.db.

### Triggers tab (low-friction creation)
- One-line quick-add: `text @ time [recur]` parsed inline (e.g. "gym @ 17:30 mon,wed,fri"), plus a fallback form. One-time and recurring (daily/weekly/custom days).
- Each trigger: mode override (auto / force-strong / force-soft), escalation on/off, check-in on/off.
- **Import lane (future, background):** `trigger_source` field on every trigger (`manual | gmail | gcal | ...`) + a staging "Suggested triggers" inbox where connectors deposit candidates for one-click accept. Schema reserved now, connectors later.

### Calendar tab
- Month/week views. Overlays: (a) events from multiple Google Calendars (read), (b) all tasks with deadlines (from rules/tasks db, distinct styling).
- Write path: create/edit events on ONE designated primary Google Calendar only.
- Google API access (OAuth, token cache) lives entirely in nudge-app; svc never touches it. Offline = tasks still render, Google layers grey out.

### Settings tab
- Notification mode tuning (per-mode colors, sounds, escalation ladder, on-task fade time), productive-app list for on/off-task classification, snooze defaults, AW endpoint, calendar account.

## 3. Data Model Changes

- Tasks graduate from `rules.toml` nudge entries to a `tasks` table in the DB (title, desc, deadline, type, recur spec, trigger mode, source, gcal_event_id nullable). rules.toml keeps global config (anchor style, ladders, snooze defaults). svc reads tasks read-only; app is the writer; reload event signals changes.
- sessions.db additions: notification mode fired, on/off-task classification result, click-throughs.

## 4. Build Phasing

1. **P0 (svc, no GUI):** two-mode notification rendering in overlay.rs (strong panel vs. soft strip) + AW-based mode pick at edge. Reuses escalate.rs plan.
2. **P1:** nudge-app shell — 3-pane layout, Planner tab (week + variable lists), quick-add triggers, task page, notification click-through.
3. **P2:** Calendar tab read-only (multi-calendar display), tasks overlaid.
4. **P3:** Calendar write to primary; Suggested-triggers inbox schema live.
5. **P4:** Gmail/Calendar auto-trigger connectors; nudge-draft integration.

## 5. GUI Stack Decision (for nudge-app only)

Chosen: **Tauri v2 (Rust backend + web frontend, e.g. Svelte)** for nudge-app.
- Shares Rust workspace + nudge-core types; WebView2 is system-provided on Win11 (no Electron bundle); ~10× lighter than Electron; calendar/list UIs are dramatically faster to build in HTML/CSS than egui/Slint.
- Acceptable because nudge-app is **on-demand, not resident** — the strict budget applies to nudge-svc, which remains pure Win32.
- Rejected: egui (immediate-mode = continuous repaint pressure + weak text/layout for calendar grids), raw Win32 (months for this feature set), Electron (footprint).

## 6. Addendum (2026-07-12): Task Tools, Time Estimates, Tool-Aware Check-ins, Pause

### 6.1 Budget amendment (explicit, load-bearing)
Tool-aware check-ins require periodic AW sampling. Amended rule: **while a task is in progress (STARTED) and not paused**, svc may arm a sampling edge every 5 min (single-timer model unchanged: next edge = min(schedule, escalation, sample, pause-expiry)). Zero sampling when IDLE, paused, or on break. Each sample = one AW read + list compare; no hooks, no sub-minute timers.

### 6.2 New task parameter: Tools (software required)
- **Selector UI (new-task form):** searchable dropdown, multi-select → selected tools render as removable chips. List populated from AW app buckets (trailing 90 days usage, cached in DB at calendar-refresh cadence), sorted most→least used; typing filters by substring on app name. Favorites pinned above the usage-sorted list; hidden tools excluded unless a "show hidden" toggle is flipped inside the dropdown. "Add tool manually" row at bottom for exes AW hasn't seen.
- **Settings → Tools tab:** three-way classification per known app: **Favorite** (pinned top), **Normal**, **Hidden** (never shown in selectors, but recoverable — hiding ≠ deleting). Separate **Not-Tools list**: seeded by recommendation (high AW usage + never attached to any task), manually editable. **Not-tools are consulted only after the user answers "Yes" at an off-task check-in (§6.5)** — they never independently trigger notifications. Per-task, transient **Ignore list** (§6.4) also visible here read-only.

### 6.3 New task parameter: Estimated time + dynamic fill
- Every task requires `estimate_minutes`. Everywhere a task name renders (lists, sidebar, notifications, calendar), append `(X of Y min)` / `(X.X of Y h)` with a slim progress-fill bar.
- `X` = accumulated on-task time from AW (foreground ∈ task tools while task active), computed **lazily at render/edge time** from AW history + sessions.db — no live ticking. Stored as `logged_minutes` updated at sample/check-in/ack edges.

### 6.4 Tool-aware ON-TASK check-in (default every 30 min, per-task customizable)
Trigger: sampling shows current tools ∉ tool list of any task due ≤48 h, while a task is nominally in progress.
- **Presentation:** centered box, must be interacted with to dismiss (deliberate exception to the soft on-task style; still never steals keyboard focus from the foreground app and other windows stay clickable around it).
- **Content:** "What are you working on?" → scrollable list of tasks due ≤48 h, top 3 visible without scrolling (sorted by deadline), + "New task…" (abbreviated new-task form: title, deadline, estimate, tools — same selector, prefilled with tools used since last check-in).
- **After selection:** classification screen — tools used since last check-in listed with per-tool choice: add to task's **tool list** / global **not-tool list** / task-scoped **ignore list** (suppresses notifications for that tool for the remainder of this task only).

### 6.5 Tool-aware OFF-TASK check-in (default: 5 consecutive min on non-task tools)
Trigger: sampling shows ≥5 consecutive min (customizable) of tools ∉ active task's tool list.
- "Still working on <task>?" **Yes** → classification screen (§6.4) for tools used since last check-in (this is the only point where the not-tools list is applied).
- **No** → gentle transition copy first — "It's okay. Here's what's on your plate — pick one to start, or take a break." (tone: non-judgmental, encouraging; variants customizable) — then the deadline-window task list (§6.8, up to 12 tasks) + **"Take a break"**.
- Select task → launch that task's required tools if not already running (ShellExecute; skip already-running exes).
- Take a break → 10/15/20/25 min or custom → all notifications paused until break ends (single pause-expiry edge).
- **Expanded list presentation (deliberate):** the notification window grows vertically to fit up to 12 uniform-height task rows (as if the 3-row list stretched) — the volume of visible work is intentionally confronting. Sorted by deadline, soonest first; top 3 always visible without scrolling, remainder scrolls if >12 would overflow the screen.

### 6.6 Pause
- **Tray:** new "Pause" item with hover submenu: 20 min / 30 min / 45 min / 1 h / 1.5 h / Custom. Pauses ALL notifications + sampling; one absolute pause-expiry edge resumes. Tray icon shows paused state.
- **Calendar-linked pause:** checkbox in the bottom-right corner of every event card (including Google-pulled events). Checked → auto-pause for the event's planned duration (pause edges armed from event start/end). Checkbox state stored locally keyed by event id (never written back to Google).

### 6.7 Google Calendar refresh policy (performance cap)
Pull events only: (1) on nudge-app launch, (2) every 24 h since last successful refresh while running, (3) manual **Refresh** button in the calendar tab's top control row (next to month navigation), with last-refreshed timestamp shown. No webhooks, no background sync process; AW usage-cache refresh piggybacks on the same cadence.

### 6.8 Dynamic deadline window (replaces fixed 48 h inclusion rule)
Inclusion horizon scales with **remaining work** = `estimate_minutes − logged_minutes` (the dynamic tracker value, not the static estimate alone — as X accumulates, remaining shrinks and the window tightens back toward 48 h):

| Remaining work | Shown when due within |
|---|---|
| < 3 h | 48 h |
| ≥ 3 h | 72 h |
| ≥ 6 h | 96 h |
| ≥ 12 h | 168 h (1 week) |

Applies everywhere "due ≤48 h" appeared (§6.4, §6.5, sidebar top). Sooner deadlines always sort first regardless of window that admitted them.

### 6.9 Task-row styling + modular list logic
- Row color encodes completion level (`logged/estimate` bands); band colors customizable in a new **Settings → Style tab**.
- Not-started default: white background, black outline. Outline turns **red** when not started and <24 h remain to deadline.
- **Modularity (load-bearing):** selection/sort/window logic = pure function in nudge-core (`task_window.rs`: `(tasks, now, config) -> ordered display list + style class per row`), shared by svc popups and nudge-app; presentation = one reusable frontend component (`TaskListPanel`) + one svc render routine consuming the same output. Expected to be revised often — keep zero business logic in the rendering layer.
