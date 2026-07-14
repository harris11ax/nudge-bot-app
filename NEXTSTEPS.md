# NEXTSTEPS.md
<!-- Boundary: forward-looking task queue only. Session history lives in HISTORY.md.
     Repo is live at github.com/harris11ax/nudge-bot-app — see "GitHub Workflow" below
     for branch/commit/PR conventions now that this is a real remote, not just a local tree. -->

Last completed: session 41 — 10e Gmail/GCal connectors: `gmail.rs` (gmail.readonly) + `connectors.rs`
(GCal events + actionable Gmail → suggested_triggers, deduped) + `run_connectors` command + Triggers-tab
"Scan Gmail + Calendar" button. Full history: [HISTORY.md](HISTORY.md).

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

- [ ] **1 — 11–12 — UI addendum (task tools, check-in flow, dynamic deadline windows, style settings)** | **Opus** | Planning run required.
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
