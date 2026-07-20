# Opus planning prompt — NEXTSTEPS Step 1 (§11–12 UI addendum)

You are planning, not coding. Produce a phased implementation plan for the next
functional milestone of nudge-bot, then stop for review. Do **not** write
implementation code in this run.

## Context to read first (in this order)
1. `README.md` — hard resource budget + scope exclusions (non-negotiable constraints).
2. `NEXTSTEPS.md` — Step 1 is the target; note the §6.1 budget amendment.
3. `UI-PLAN.md` §6.1–§6.9 — the actual spec for this step (task tools, time
   estimates, ON/OFF check-ins, Pause, GCal refresh cap, dynamic deadline
   window, task-row styling).
4. `ARCHITECTURE.md` — state machine + escalation edges.
5. `COMPONENTS.md` — module registry.
6. Current code state (verified functional this session): `crates/nudge-core/src/`
   (`state.rs`, `schedule.rs`, `tasks.rs`, `escalate.rs`), `crates/nudge-svc/src/main.rs`
   (single-thread GetMessage loop, single edge timer), `crates/nudge-ctl/`,
   `crates/nudge-app/src-tauri/` (Tauri GUI, OUT of workspace).

## Verified baseline (do not re-plan these — they already work)
- Workspace builds green; release binaries run.
- svc boots → seeds rules.toml → probes ActivityWatch → enters active window →
  fires escalating prompt → logs to sessions.db → quits on named event.
- `Recur::Once` tasks fire via deadline date-anchor (commit 72f43bf).
- ctl verbs: status/log/validate/reload/quit/anchor/nudge add|rm|ls.

## Load-bearing constraints (any plan violating these is wrong)
- **Zero polling.** One armed waitable timer = min(schedule, escalation, sample,
  pause-expiry). No sub-minute timers, no background tick.
- **5-min AW sampling edge permitted ONLY while STARTED and not paused** (§6.1).
  Zero sampling when Idle, paused, or on break.
- **`logged_minutes` computed lazily at render/edge time** from AW history +
  sessions.db — never live-ticking.
- **Single `task_window.rs` pure module in nudge-core** — `(tasks, now, config)
  -> ordered display list + per-row style class`. Zero business logic in svc
  render routines or the frontend `TaskListPanel`. This module currently does
  NOT exist and must be introduced.
- No network I/O in svc/ctl (GCal + LLM live only in app/nudge-draft).
- RAM <30 MB svc; every timer/window paired create/destroy on state entry/exit.

## Deliverable — the plan must contain
1. **Scope cut for "functional ASAP"**: rank §6.1–§6.9 sub-features by what
   unblocks daily use vs. what is polish. Recommend the smallest vertical slice
   that lets the user run STARTED tasks with working ON/OFF check-ins and Pause.
   Explicitly defer the rest with reasons.
2. **State-machine deltas**: new States/Events/Effects for STARTED sampling
   edge, Pause (+ expiry edge), check-in Yes/No branches, break. Show how they
   fold into the existing single-edge `next()` in `state.rs` without adding a
   second timer.
3. **Data-model deltas**: `tasks` table columns (tools list, estimate,
   logged_minutes, ignore list, pause state), not-tools + tool-classification
   storage. Migration approach for existing sessions.db.
4. **`task_window.rs` signature + the exact dynamic-deadline-window table**
   (§6.8) and row-style rules (§6.9, incl. red-outline <24h not-started).
   Name the shared consumers (svc popup render + app `TaskListPanel`).
5. **svc vs. app responsibility split** per feature (writer = app, read-only =
   svc, reload-signal boundary).
6. **Ordered phase list**, each phase independently buildable/testable, ≤1
   NEXTSTEPS step each, with a concrete verification step per phase (how to prove
   it fires/samples/pauses correctly without a real 30-min wait — e.g. injectable
   `now`, short test intervals).
7. **Budget-risk callouts**: any place the plan flirts with the zero-polling /
   30 MB / no-network rules, with the compliant alternative.

## Output format
Numbered phases, each: goal · files touched · state/data deltas · verification ·
risk. End with a one-paragraph "smallest shippable slice" recommendation and the
single NEXTSTEPS step to tackle first. Then stop — await approval before coding.
