# NEXTSTEPS.md
<!-- Boundary: forward-looking task queue only. Session history lives in HISTORY.md
     (interim home until a GitHub repo exists to hold it as issues/wiki instead). -->

Last completed: session 38 — hardened nudge-app launch (`scripts\launch-both.ps1` always rebuilds
via `tauri build`). Full history: [HISTORY.md](HISTORY.md).

## Next steps
- [ ] **1 — 10c — Calendar WRITE (primary only)** | **Sonnet** | ~2h
  Primary-calendar picker (`set_primary_calendar`), `events.insert`/`update` on primary only, create/edit
  dialog with optimistic local cache write then push. Scope `calendar.events`. Depends on 10b (done).

- [ ] **2 — 10d — Suggested-triggers inbox** | **Sonnet** | ~2h
  App-only `suggested_triggers` table (schema live, empty). Triggers-tab "Suggested" section with source
  badge; Accept → insert `tasks`(trigger_source) + signal_reload; Dismiss → mark. Depends on 10a (done).

- [ ] **3 — 10e — Gmail/GCal connectors** | **Opus** | ~1d
  `gmail.rs` (gmail.readonly) + `connectors.rs`: GCal events + email candidates → suggested_triggers,
  dedup vs tasks.gcal_event_id + pending. Optional nudge-draft LLM to phrase title / infer time.
  BLOCKED end-to-end on deferred once/deadline-only task firing (per-task done-flag). Depends on 10d.

- [ ] **4 — 11–12 — UI addendum (task tools, check-in flow, dynamic deadline windows, style settings)** | **Opus** | Planning run required.
  Details in NEXTSTEPS.md §11–§12 (task-tool selector, AW-usage sort, ON/OFF check-in popups,
  tray Pause menu, calendar-event auto-pause, GCal refresh caps). Budget: 5-min AW sampling edge
  permitted ONLY while STARTED+unpaused. Dynamic window scale by remaining work. Off-task list
  ≤12 rows + not-started red-outline at <24h. Single `task_window.rs` core module, zero UI logic.
  Gentle "It's okay" copy on No.

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

## Open Decisions
- Max snoozes per prompt? (currently unlimited, each just logged.)
- Check-in is global-only for now; per-window override (`[[nudge]].checkin_after_secs`) if wanted later.
- UI: productive-app list global-only for P0, per-task later? Task-type taxonomy for checkbox filters — user to supply initial list.
