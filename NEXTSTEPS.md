# NEXTSTEPS.md
<!-- Boundary: forward-looking task queue only. Session history lives in HISTORY.md.
     Repo is live at github.com/harris11ax/nudge-bot-app — see "GitHub Workflow" below
     for branch/commit/PR conventions now that this is a real remote, not just a local tree. -->

Last completed: session 53 — Step 1 **DONE** (fixed session-52 review findings: task_id carry through
CheckIn/Choosing/Paused, re-Pause was_started preservation, logged_minutes sub-minute accrual). 148
workspace tests green, not committed. Next: step 2 (Sonnet — verify tray Pause submenu with a real
click). Full history: [HISTORY.md](HISTORY.md).

## Next steps
- [x] **10e — Gmail/GCal connectors** | **Opus** | ~1d — DONE session 41.
  `gmail.rs` (gmail.readonly) + `connectors.rs`: GCal events + email candidates → suggested_triggers,
  deduped vs tasks.gcal_event_id + pending. nudge-draft LLM title/time pass DEFERRED (binary crate, needs
  lib split). END-TO-END UNBLOCKED (feature/once-task-firing): `Recur::Once` tasks now fire via a
  date-anchor — `schedule::Win` carries `on_date` (the deadline instant); a once task is live only on the
  day whose local midnight..+24h contains its deadline, so it fires exactly once (the date passes) with NO
  completion flag needed, and past-dated once tasks contribute no future edge. `accept_suggested_trigger`
  derives `minutes` (local time-of-day) from the deadline via chrono so the synthesized window opens at the
  right wall-clock time. Deadline-only tasks (no time-of-day) and undated-once suggestions remain deferred.

- [x] **1 — 11–12 — UI addendum (task tools, check-in flow, dynamic deadline windows, style settings)** | **Opus** |
  DONE session 43 — the Step-1 slice (PLAN-step1 Phases 1–4) is complete. Tier B/C remain deferred; see
  "Next up" below for what they became.
  Details in NEXTSTEPS.md §11–§12 (task-tool selector, AW-usage sort, ON/OFF check-in popups,
  tray Pause menu, calendar-event auto-pause, GCal refresh caps). Budget: 5-min AW sampling edge
  permitted ONLY while STARTED+unpaused. Dynamic window scale by remaining work. Off-task list
  ≤12 rows + not-started red-outline at <24h. Single `task_window.rs` core module, zero UI logic.
  Gentle "It's okay" copy on No.
  - Progress (PLAN-step1.md phases): **P1 ✅** `task_window.rs` pure `display_list` (§6.8/§6.9), 59 core tests.
    **P2 ✅ (2026-07-14)** data-model + additive migration: `tasks.estimate_minutes`/`logged_minutes`,
    `task_tools`/`app_classes`/`app_usage` tables, `set_logged_minutes` (sole svc write), `task_tools()`/
    `app_class()` readers, `PRAGMA user_version = 2`; svc persist tests green.
    **P3 ✅ (2026-07-15)** STARTED sampling edge + lazy logged_minutes: `EdgeKind::Sample`; `Started` gains
    `sample_at`/`off_task_since`; `arm_started` collapses the two runtime edges (check-in, sample) to their
    earliest before the schedule merge, so the single-armed-timer invariant holds. Sampling is opt-in
    (`[escalation] sample_secs = 0` default, `off_task_secs = 300`); a continuous off-task run ≥ threshold
    raises the drift check-in, returning on-task resets it. svc: `sample_due` gate + `foreground_on_task`
    (task_tools → productive_apps fallback; `ignore` kind, AW-down, and nothing-configured all read on-task)
    + one-cadence `logged_minutes` accrual at the edge. 106 workspace tests green. Not committed.
    **P4 ✅ (2026-07-15)** OFF-task check-in Yes/No + task list + Take-a-break + Pause (§6.5/§6.6):
    `CheckInKind::{Periodic,OffTask}`; `Choosing`/`Paused`/`Break` states; `ShowTaskList`/`HideTaskList`;
    `EdgeKind::{PauseExpiry,BreakExpiry}`; `Buttons` as core data; new svc `tasklist.rs` painting
    `display_list` rows verbatim; tray Pause/Resume with `[escalation] break_secs`/`pause_secs`.
    Pause is handled *before* the `!in_window` collapse and arms a bare expiry edge — that is what makes a
    pause outlive its window and stay silent. 122 workspace tests green. Not committed.

- [x] **2 — Drive P4's UI end-to-end once** | **Sonnet** | session 44 — ran, found 2 blocking bugs, NOT clean.
  Method: `$env:LOCALAPPDATA` pointed at a scratch dir (never touched the real
  `%LOCALAPPDATA%\nudge-bot\rules.toml`), scratch `rules.toml` with an always-live window and
  `sample_secs=15`/`off_task_secs=20`/`break_secs=30`/`pause_secs=30`, one task seeded into
  `sessions.db` directly (no `nudge-ctl` task-add verb exists — only rules/status/log). Built and ran
  `nudge-svc.exe`/`nudge-ctl.exe` for real.
  - **Confirmed working**: anchor strip renders (`BG_L0`, correct text, `WS_EX_TOPMOST`/`NOACTIVATE`
    strip at the configured height/position). Button *hit-testing* works — a synthetic click at the
    Start button's computed rect (`button_rect`) correctly drove `prompting → started`
    (`nudge-ctl status`/`log` confirmed the edge).
  - **BUG (blocking)**: the `[Start][Snooze][Skip]`/`[Yes][No][Break]` buttons never paint. Screenshot
    at high zoom over the whole button-cluster rect (right 192px of the strip) shows flat background,
    no button face/frame/label — `anchor_proc`'s `WM_PAINT` button-drawing loop
    (`crates/nudge-svc/src/overlay.rs:339-345`) runs but produces no visible output, even though the
    *geometry* (`button_rect`) is correct enough for hit-testing to work. Likely a GDI object/paint-order
    bug (brush not selected into `hdc`? draw call ordering vs `EndPaint`?) — needs a focused repro, not
    diagnosed further this session. A real user cannot see what to click; this alone blocks calling P4 done.
  - **UNVERIFIED (not blocking, ran out of budget)**: off-task drift check-in never fired in ~7 min of a
    Started task sitting on an off-task foreground app (`sample_secs=15`, `off_task_secs=20` — should
    have fired within ~35s). No edge-log entry beyond `started`. Could be a real second bug in
    `sample_due`/`EdgeTimer` (session 43 wrote both same-day, untested until now), or could be an
    artifact of driving the click via synthetic `PostMessage` instead of a real OS click (message loop
    may process input differently). Task list render, Yes/No, Take-a-break, and tray Pause/Resume were
    **not exercised** — blocked on reaching the check-in first.
  - **Next**: fix the button-paint bug first (small, isolated — `overlay.rs` paint path). Then re-run this
    same scratch-config method and get an actual off-task check-in to fire (try a real mouse click instead
    of `PostMessage`, or add an `eprintln!` at the `sample_due`/`arm_started` call sites to confirm the
    timer is arming) before touching Tier B.

- [x] **2b — Fix invisible prompt buttons + verify off-task check-in fires** | **Sonnet** | DONE session 46.
  **Both items resolved — neither was a code bug.**
  1. Button paint "bug" was a screenshot artifact: the strip is `WS_EX_LAYERED`, and BitBlt-based capture
     (`Graphics.CopyFromScreen`, session 44's screenshots) omits layered windows unless `CAPTUREBLT` is set.
     `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)` on the `NudgeAnchor` hwnd shows buttons render perfectly
     (face + frame + labels, both `[Start][Snooze][Skip]` and `[Yes][No][Break]`). `overlay.rs` untouched.
  2. Off-task check-in fires correctly. Session 44 never saw it for two stacked reasons, both environmental:
     (a) the 30s `SetWaitableTimerEx` tolerable-delay makes each sample edge land up to `sample_secs+30s`
     late (a 15s cadence really ticks ~45s) — patient waiting shows the timer is fine; and (b) with the
     test's rules-window unlinked to any task, `window_task_id` is None and an empty `productive_apps`
     means `foreground_on_task` reads on-task by design ("nothing configured → never nag"), so no drift
     accrues. Adding `[classify] productive_apps` to the scratch config made the full path run.
  Driven end-to-end live (session 46): prompting→Start→started, sample edges, off-task drift check-in
  (`[Yes][No][Break]` strip), Yes→started, No→choosing + task list render ("What's actually due", red
  outline <24h row, Take-a-break button), Take-a-break→break→expiry→started/resumed. All verified via
  sessions.db edges + PrintWindow captures. 122 workspace tests green. **Tray Pause/Resume remains
  unexercised** (needs a real tray-menu click; synthetic input can't reach it — fold into Tier B testing).
  Method notes for future live-driving: `PrintWindow` + `PW_RENDERFULLCONTENT` for screenshots of the
  layered strip (works even at the lock screen); `PostMessage(WM_LBUTTONDOWN)` drives buttons fine;
  timer coalescing means allow +30s on every edge, or temporarily shrink the tolerance in
  `timers.rs::arm_absolute` while testing.
  UNSKIPPABLE before Tier B, found by Step 2 (session 44). Two items, do both in one pass since both
  need the same scratch-config driving method already set up in Step 2's notes:
  1. Button paint bug: `crates/nudge-svc/src/overlay.rs` `anchor_proc` `WM_PAINT` handler draws buttons
     (`button_rect` + `FillRect`/`FrameRect`/`DrawTextW`, lines ~336-347) but nothing appears on screen,
     even though the same rects hit-test correctly on click. Diagnose (GDI brush/object selection, draw
     order, or a clipping rect issue) and fix.
  2. Off-task check-in never observed firing (`sample_secs=15`/`off_task_secs=20` in test config, no
     edge after 7 min of Started). Confirm whether `sample_due`/`arm_started`/`EdgeTimer` actually arms
     and fires under a **real** mouse click (not `PostMessage`) — Step 2 used a synthetic click to work
     around the paint bug, which may itself be the reason sampling never triggered.
  Once both are clean, finish driving the rest of Step 2's checklist (task-list render, Yes/No, Take-a-
  break, tray Pause/Resume) before starting Tier B.
  - **Session 45 attempt — NOT resolved, re-pick this up next session.** Two background-agent delegation
    attempts both failed: the agents got stuck in a confused loop each claiming the other was doing the
    work, with zero real tool calls — do not delegate this step to a subagent again; drive it directly.
    Doing it directly got as far as: workspace builds clean (`cargo build -p nudge-svc -p nudge-ctl`).
    Scratch-config repro method reproduced from Step 2's notes and confirmed working: `$env:LOCALAPPDATA`
    → a scratch temp dir, `rules.toml` with an always-live window + `sample_secs=15`/`off_task_secs=20`/
    `break_secs=30`/`pause_secs=30`, one task seeded into the scratch `sessions.db` via Python's `sqlite3`
    module (no `sqlite3` CLI installed in this env — use Python, and note `sqlite3.connect` needs a
    Windows-style `C:/...` path, not the Git-Bash `/c/...` form, or it fails to open the file).
    **New finding this session**: the default hotkey (`ctrl+alt+N`) collides with something already
    registered on this machine and panics `nudge-svc` on launch (`RegisterHotKey ... already registered`)
    — the scratch `rules.toml` must use a different combo (used `ctrl+alt+shift+Y` successfully). Worth
    calling out in Step 2's/2b's method notes for whoever runs this next.
    **Blocked**: could not get visual confirmation of the button-paint bug. `nudge-svc.exe` was running
    live with the scratch config, but a `computer-use` `request_access` call for desktop screenshot access
    was denied (`File Explorer` → `user_denied`), and `nudge-svc`'s window is a raw `WS_POPUP` tool window
    with no taskbar identity anyway, so it's unclear the app-based permission model can even target it.
    Static re-review of `overlay.rs`'s `WM_PAINT` handler (brush create/fill/frame/delete order, RECT
    construction, tag packing/unpacking between paint and hit-test) found nothing conclusively wrong —
    textbook GDI usage, consistent geometry between the two paths. **Live screen access is required to
    make progress here**; static reading alone hasn' found the bug. Next session: either get computer-use
    desktop access granted up front, or have the user manually run the scratch-config repro and describe/
    screenshot what they see, before touching `overlay.rs` again.

- [x] **3 — Tier B: ON-task check-in (§6.4) + tool classification screen + rich Tools selector (§6.2)** | **Opus** |
  **Planning DONE session 47 → [PLAN-step3.md](PLAN-step3.md).** Decisions LOCKED (PLAN-step3 §1): (A)
  since-last-checkin = accumulate sampled apps in svc scratch; on-task cadence is a floor, user-configurable
  `[escalation] ontask_checkin_secs` (default 1800, set to seconds for testing); ordering optimises for
  fastest functional state. Execute the five phases in PLAN-step3 §2, one per session:
  - **P1 ✅ (2026-07-16, session 48)** core: `CheckInKind::OnTask` live, `Started.ontask_at` third runtime
    edge in `arm_started`'s 3-way merge (floor semantics: on-task tick = silent re-arm), `ScheduleCtx.
    {any_task_on_task,ontask_secs}`, `[escalation] ontask_checkin_secs` (default 1800 — ON for existing
    configs, silent unless drifted). svc: `ontask_due` gate, `any_task_on_task()` union compare,
    `seen_tools` accumulator (fills at probes, clears on check-in resolution — P2 consumes),
    `Buttons::TaskList` rendered via Yes/No/Break stub (real picker = P2). 133 tests green. Not committed.
  - **P2 ✅ (2026-07-16, session 49)** svc classification screen: `State::Classifying`,
    `ShowClassify`/`HideClassify`, `Event::{PickTask,Classify,ClassifyDone}` + `ClassifyChoice`;
    `classify.rs` overlay ([Tool][Not a tool][Ignore] per row, Done footer, auto-Done on last row);
    OnTask picker = the task list (rows send `PickTask(task_id)`; §6.5 Choosing pick still resolves
    like Start); OffTask Yes classifies only when tools accumulated + window has a task (else pre-P2
    behaviour). Persistence via new svc writers `add_task_tool`/`set_app_class` at the main-loop seam.
    143 tests green. Live drive deferred to P5. Not committed.
  - **P3 ✅ (2026-07-16, session 50)** nudge-app: `ToolSelector.svelte` chip selector (usage-sorted,
    favorites pinned, hidden toggle, manual add) in the Triggers form + Estimate field; Settings shell
    with Tools tab (favorite/normal/hidden/not-tool, candidates, ignores view) + Style tab (band colors
    → `meta.style_bands`); 10 new tauri commands; C.5 gap closed via new `aw_usage.rs` 90-day AW
    aggregation behind `refresh_app_usage` (24h cap). 43 app tests + vite build green. Not committed.
    Deferred: renderers don't read `style_bands` yet.
  - **P4 ✅ (2026-07-16, session 51)** Tier-C switch + tray Pause submenu: `Started` gains `task_id`
    (window-seeded; a list `PickTask` rebinds it and emits new `Effect::LaunchTools` — svc launches
    `task_tools(kind='tool')` minus running exes via Toolhelp+ShellExecute); sample compare/accrual
    follow the carried task. Tray Pause → duration submenu (20m–1.5h + rules default,
    `TrayCmd::Pause(Option<i64>)`); paused-state tray icon (grey pause bars). Custom pause input
    deferred. 145 workspace tests green. Not committed.
  - **P5 ✅ (2026-07-16, session 52)** verify: 145 tests + svc/ctl/app builds green; live drive of
    on-task check-in → task-list pick → classification → `task_tools`/`app_classes` persisted (both
    synthetic and real-mouse). Review (low): 4 non-blocking findings logged in HISTORY session 52.
  Deferred from Step 1 on purpose (PLAN §1): §6.4 overlaps §6.5's mechanics but needs the classification UI,
  and building it before that UI exists means building it twice. `CheckInKind` gains its `OnTask` variant
  here — that is the variant PLAN §2 named and P4 deliberately left out as dead code.
  Also lands here: a row click switching the live window to the *picked* task (P4 resolves the check-in but
  ignores the row's `task_id` — PLAN Tier C), and a tray Pause *submenu* of durations.

- [x] **1 — Fix session-52 review findings (task carry + pause + accrual)** | **Sonnet** | DONE session 53.
  (a) `State::CheckIn`/`Choosing`/`Paused` gained `task_id: Option<i64>`, threaded through resolution
  arms and `quiet_tick`/`resume` (state.rs ~990-1097) so a Tier-C row-pick survives a check-in/pause
  round-trip instead of resetting to `ctx.window_task_id`.
  (b) Re-Pause while already `Paused` now preserves the existing `was_started`/`task_id` instead of
  recomputing via `was_live()` (always false for `Paused`) — new `task_id_of()` helper, state.rs
  ~1027-1037.
  (c) `accrue_logged_minutes(&mut i64, i64) -> i64` extracted in main.rs; sub-minute sample cadences
  now accrue a carried-seconds remainder instead of truncating to 0 minutes per tick.
  148 workspace tests green (up from 145), builds clean. Not committed.

- [ ] **2 — Verify tray Pause submenu with a real tray click** | **Sonnet** | ~30m
  Unexercised since P4 (synthetic input can't reach the tray menu). Step 2b scratch-config method +
  user clicks the tray: pick 20m → single `PauseExpiry` edge at +20m, grey paused icon, Resume works.
  Do AFTER step 1 so the re-Pause fix is also covered.

- [ ] **3 — Renderers consume `meta.style_bands`** | **Sonnet** | ~1h
  Deferred from P3: StyleTab writes band colors to `meta.style_bands` but svc tasklist/overlay renderers
  still use hardcoded band colors. Read the setting at rules/task load; fall back to defaults.

- [ ] **4 — Deadline-only tasks (no time-of-day) + undated-once suggestions** | **Sonnet** | ~2h
  Deferred from 10e: a once task with a deadline date but no `minutes` produces no window
  (`Win::from_task` returns None). Design a sensible default window; also let
  `accept_suggested_trigger` handle undated suggestions. (Escalate to Opus if scheduling semantics
  get contentious.)

- [ ] **5 — Custom pause duration input** | **Sonnet** | ~1h
  Deferred from P4: tray submenu offers fixed durations only. Add a free-input surface —
  likely a Settings field or small prompt window.

- [ ] **6 — nudge-draft LLM title/time pass** | **Opus** | ~1d
  Deferred from 10e: needs the binary crate split into lib+bin first, then an LLM pass that
  drafts task titles/times from Gmail/GCal candidates. Architectural (crate split + API integration).

## Session model: Sonnet (default) | Opus gate on planning/complex design | Haiku for trivial tasks
- Read NEXTSTEPS.md at start. Scan for incomplete steps:
  - **Priority 1**: Find an unskippable Sonnet step → proceed without prompting.
  - **Priority 2**: If no unskippable Sonnet steps, find ANY Sonnet step → proceed without prompting.
  - **Priority 3**: If no Sonnet steps remain, then apply Opus/Haiku gates.
    - If marked Opus: state "Opus required for this step" and wait for user "Opus" response.
    - If marked Haiku: state "Haiku sufficient for this step" and wait for user "Haiku" response.
- Complete ONE step per session only. Update checkbox on done.
- On completion, log the session to HISTORY.md (not this file) via the nextsteps-classifier skill.
- If all done: end routine, confirm completion.
- If NEXTSTEPS.md absent/empty: scan project for planned features, add next logical step to the list.

## GitHub Workflow
- **Remote**: `https://github.com/harris11ax/nudge-bot-app.git`, `main` tracking `origin/main` (verified linked, session 39).
- **Status quo (as of session 40)**: only one commit is actually pushed (`ade2023`, "Initial commit"). All
  work since — sessions 2–40 per HISTORY.md — exists only in the local working tree/history, uncommitted.
  Until the user asks to commit/push, treat NEXTSTEPS.md/HISTORY.md as still describing *uncommitted*
  progress; don't claim something is "on GitHub" unless `git log`/`git status` confirms it.
- **Branch strategy**: `main` is production-ready. Feature work goes on branches: `feature/description`,
  `fix/description`. One NEXTSTEPS step per branch.
- **Commit messages**: Clear, concise, reference the NEXTSTEPS step (e.g., "10c: Add primary calendar picker").
- **PR workflow**: One step per PR, opened via `gh pr create` once `gh` is available/authenticated in this
  environment (not currently — `gh` is absent from PATH here). Link the PR description to the relevant
  NEXTSTEPS step. Request review before merge.
- **Commits/pushes are still explicit-permission actions** (per this environment's safety rules) — write
  them to disk and report readiness, but don't `git add`/`commit`/`push` without the user asking in the
  same turn.
- **Issues/Wiki migration**: HISTORY.md's per-session log is a candidate for GitHub Issues (one issue per
  NEXTSTEPS step) or the repo Wiki once the user wants that split — not done yet; needs `gh` auth or a
  manual pass, and is a deliberate ask, not an assumed default.

## Open Decisions
- Max snoozes per prompt? (currently unlimited, each just logged.)
- Check-in is global-only for now; per-window override (`[[nudge]].checkin_after_secs`) if wanted later.
- UI: productive-app list global-only for P0, per-task later? Task-type taxonomy for checkbox filters — user to supply initial list.
