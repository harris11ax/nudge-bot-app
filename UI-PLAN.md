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

- **Left sidebar:** collapsible (icon-only when collapsed). Tabs: Planner, Calendar, Tasks, Settings, Log. (Nomenclature: the former "Triggers" entity/tab is renamed **Task/Tasks** throughout the app — see §7.1.)
- **Right sidebar:** always visible, user-resizable width (persisted). Content = to-do list; **next deadline pinned at top** (name, countdown, one-click Start). Below: compact task list sorted by deadline.
- **Content area:** renders active tab.

### Planner tab
- **Week list:** tasks with deadline ≤ 7 days OR missed deadline (missed styled distinctly, sorted first).
- **Variable list:** all other tasks, filters: time range (2 weeks / 1 month / custom), keyword search over title+description, checkbox task-type filters. Filter state persisted.
- Task page (opened by notification click or list click): title, description, **Deadline**, type, task(s), history from sessions.db. Includes **Launch tools** controls (§7.4).

### Tasks tab (low-friction creation)
Ordering (top → bottom): **Quick Add** first, then the task list, then **Suggested** last (§7.1).
- **Quick Add (top):** one-line quick-add: `text @ Deadline [recur]` parsed inline (e.g. "gym @ 17:30 mon,wed,fri"), plus a fallback form. One-time and recurring (daily/weekly/custom days). The start-time field is labelled **Deadline** everywhere (§7.1).
- Each task: mode override (auto / force-strong / force-soft), escalation on/off, check-in on/off.
- **Suggested (bottom) / import lane:** `task_source` field on every task (`manual | gmail | gcal | ...`) + a staging **"Suggested"** inbox where connectors deposit candidates for one-click accept. A task already tied to a calendar Event is **excluded** from Suggested (§7.2).

### Calendar tab
- Month/week views. Overlays: (a) events from multiple Google Calendars (read), (b) all tasks with deadlines (from rules/tasks db, distinct styling).
- Write path: create/edit events on ONE designated primary Google Calendar only.
- Google API access (OAuth, token cache) lives entirely in nudge-app; svc never touches it. Offline = tasks still render, Google layers grey out.

### Settings tab
- Notification mode tuning (per-mode colors, sounds, escalation ladder, on-task fade time), productive-app list for on/off-task classification, snooze defaults, AW endpoint.
- **Tabbed Settings:** General, Tools (§6.2), Style (§6.9), and a dedicated **Calendar** tab (§7.3) holding all calendar/Google account settings (moved out of General).

## 3. Data Model Changes

- Tasks graduate from `rules.toml` nudge entries to a `tasks` table in the DB (title, desc, deadline, type, recur spec, mode, `task_source` (renamed from `trigger_source`, §7.1), `gcal_event_id` nullable = Event binding §7.2). `task_tools` rows carry an optional `url` for web tools (§7.4 launch). rules.toml keeps global config (anchor style, ladders, snooze defaults). svc reads tasks read-only; app is the writer; reload event signals changes.
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

## 7. Addendum (2026-07-18): Tasks nomenclature, Task↔Event binding, Settings/Calendar, Tools launch & attribution

### 7.1 Nomenclature: Trigger → Task, start time → Deadline
- **Global rename:** the user-facing entity/tab formerly called **Trigger/Triggers** is now **Task/Tasks** everywhere it surfaces (left-sidebar tab, headers, form labels, notification copy, Suggested inbox). Framing: the app's job is to get the user's attention and remind them to complete their **Tasks**.
- **Deadline label:** the field previously shown as the task's *start time* is labelled **Deadline** consistently in every surface (Quick Add, task page, calendar, sidebar, notifications). **Decision (2026-07-18): field-semantics change, not display-only** — the primary user-facing task time is the hard `deadline` (unix, already the sole key of `display_list`); the fire `minutes` is *derived* from it (mirrors step 4 deadline-only tasks + CSV import). Forms bind to `deadline`; `minutes` follows.
- **Status (session 58):** backend done — DB column / DTO-JSON API / `Task` field `trigger_source`→`task_source` (both crates byte-identical, back-compat `RENAME COLUMN` migration + legacy-DB test; internal `TriggerSource` type unchanged). Remaining: Svelte relabel + Deadline-first form binding + Tasks-tab reorder.
- **Code note (non-user-facing):** internal timing terms may keep "trigger edge"/"firing edge" since those name the scheduler edge, not the entity. DB/API: rename the user-facing field concept `trigger_source` → `task_source`; keep migration back-compat. This is a rename-only pass (Contractor), no behavior change.
- **Tasks tab ordering:** **Quick Add** pinned at **top**; **Suggested** moved to the **bottom** (was inline "Import lane"). Task list sits between them.

### 7.2 Task ↔ Calendar Event binding (bidirectional)
- **Auto-tie on create:** creating a Task automatically creates a bound Calendar Event on the designated primary Google Calendar and syncs it. **Default event time = Deadline + 1 h** (event start = Task Deadline, event end = Deadline + 1 h); user may edit after. Binding stored via existing `gcal_event_id` (nullable) on the task row.
- **Edit propagation:** modifying a Task updates its bound Event (title, time, deadline) and pushes the change to the applicable Google Calendar. One writer (nudge-app) on the primary calendar only, per §57–60 write-path rule.
- **Suggested exclusion:** a Task already tied to an Event does **not** appear in the Suggested list (§7.1) — it is already committed, not a candidate.
- **Event-click menu (calendar):** clicking an event on the calendar prompts a 3-choice menu: **Add Task** / **Edit Event** / **Delete Event**.
  - **Add Task** → opens **Quick Add** with the Event's fields auto-filled (title, time→Deadline, calendar source); user completes any missing fields, and the new Task binds to that Event.
  - **Edit Event** / **Delete Event** act on the Event (and, if bound, keep the linked Task consistent — deleting an event bound to a task prompts whether to also clear the binding).

### 7.3 Settings: dedicated Calendar tab
- Split calendar/Google settings out of General into their own **Settings → Calendar** tab: Google account/OAuth connection & reconnect, designated primary (write) calendar, visible read calendars, refresh policy (§6.7) & last-refreshed timestamp / manual Refresh, default event duration (default 1 h, §7.2), and Suggested-sync toggles.

### 7.4 Tools: "Unknown" attribution, per-website resolution, launch-from-task
- **What "Unknown" is:** the Tools usage list is built from AW window buckets keyed on the `app` field (`aw_query.rs`, exe name). AW's Windows window-watcher emits **`unknown`** for the foreground app whenever it cannot resolve the process — secure desktop / lock screen, UAC / elevated windows the watcher can't read, and watcher gaps. That time is **not a real process**, which is why no 36 h process appears in the ActivityWatch dashboard (≈24 min/day of lock/secure-desktop time over the 90-day window aggregates to ~36 h). **Fix:** relabel this bucket **"Unknown (system / lock screen)"**, exclude it from selectors and Not-Tools recommendations by default (recoverable via "show hidden"), and never surface it as an attachable tool.
- **Per-website resolution inside browsers:** for browser exes (`brave.exe`, `chrome.exe`, `msedge.exe`, `firefox.exe`), attribute usage to the **actual site** rather than the browser. Source the host from the AW web-watcher bucket (`aw-watcher-web` browser extension, `url`/`title` fields) when present; fall back to the window title's host. Tools then list e.g. `brave.exe → github.com` as distinct, selectable entries. Budget-safe: parsing happens at the existing cache-refresh cadence (§6.7), no new sampling.
- **Launch a tool from a task:** on the task page, each attached tool gets a **Launch** control (and a "Launch all" for the task's tool set):
  - **Websites** → open the exact URL required for the task in the default/associated browser (bound URL stored on the `task_tools` row for web tools).
  - **Windows applications** → `ShellExecute` the exe (reuses the §6.5 launch path; skip if already running).
  - Runs on-demand from the task page (not just at off-task check-ins), so viewing a task can spin up everything needed to start it.
