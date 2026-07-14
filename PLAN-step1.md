<!-- Boundary: planning artifact for NEXTSTEPS Step 1 (§11–12 UI addendum). No implementation code. Every phase must satisfy the <30 MB / zero-polling / no-network budget. -->

# PLAN — Step 1: §6.1–§6.9 STARTED sampling, check-ins, Pause

Planning run only. No code. Awaiting approval before Phase 1.

## 0. Problem / Success / Load-bearing constraints

1. **Problem:** svc today has no notion of a task being *in progress* with tool-aware supervision. It prompts at the start edge, accepts Ack→Started, and (optionally) does a presence-only check-in. §6 wants: STARTED-mode 5-min AW sampling, ON/OFF-task tool-aware check-ins, Pause, and a shared `task_window.rs` selection module.
2. **Success:** user can run a STARTED task; svc samples AW every 5 min *only while STARTED+unpaused*; an off-task run of ≥N min raises an interactive "Still working on X?" box whose No path shows the dynamic-deadline task list + Take-a-break; Pause (tray) silences everything to one absolute edge; all of it stays single-timer, <30 MB, no network in svc.
3. **Load-bearing constraints:** one armed timer = `min(schedule, escalation, sample, pause-expiry, break-expiry)`; sampling edge exists **only** in STARTED-unpaused; `logged_minutes` computed lazily at edge/render, never ticked; `task_window.rs` is the sole owner of select/sort/window/style logic (zero logic in svc render or `TaskListPanel`); no network I/O in svc/ctl; every window/timer paired create/destroy.

---

## 1. Scope cut — smallest vertical slice for daily use

Ranked by "unblocks running a task today" vs. polish:

**Tier A — the shippable slice (do first):**
- `task_window.rs` pure module (§6.8 window table + §6.9 row styles). Everything else consumes it; it has zero budget risk and unblocks both svc and app. **Build first.**
- STARTED sampling edge (§6.1) + lazy `logged_minutes` (§6.3 the `X` value). This is the new state-machine spine; check-ins are meaningless without it.
- OFF-task check-in (§6.5) Yes/No with the No→task-list→Take-a-break path. This is the feature that actually gets the user re-engaged — the core problem in README.
- Pause (§6.6 tray only) — one pause-expiry edge. Cheap, high daily value, exercises the same merged-edge plumbing.

**Tier B — fast follow (defer with reason):**
- ON-task check-in (§6.4 "tools ∉ any task due ≤window"): overlaps heavily with §6.5 mechanics but needs the full tool-classification screen; defer until the classification UI exists so we don't build it twice.
- Tool classification screen (add-to-tool-list / not-tool / ignore-list): writer-side (app) UI; needed by both check-ins but not required to prove the *edge/sampling* machinery. Ship a stub outcome (Yes = keep going, No = list) first.
- Tools selector + AW usage-sorted dropdown + Favorites/Hidden/Not-Tools (§6.2): pure app-side data entry; the schema lands in Phase 2 but the rich selector UI is polish relative to firing correctly.

**Tier C — explicit defer:**
- Estimate progress-fill bar rendering everywhere (§6.3 visual) — needs `estimate_minutes` column (lands Phase 2) but the bar is cosmetic; `task_window.rs` emits the band, rendering can follow.
- Calendar-linked auto-pause + GCal 24 h refresh cap (§6.6/§6.7) — lives in app, touches Google client; entirely deferrable from the svc slice.
- Launch-required-tools on task select (§6.5 ShellExecute) — app-side convenience, not on the firing path.

**Reason for the cut:** the README's critical goal is bringing the list to the user at the right moment. The right moment is "drifted off-task while a task is live." That requires exactly Tier A: sampling to detect drift, the window module to pick what to show, the check-in to show it, and Pause so the user can silence it honestly. Tiers B/C are richer inputs and cosmetics layered on a correct spine.

---

## 2. State-machine deltas (fold into single-edge `next()`)

New `EdgeKind` variants (in `lib.rs`): `Sample`, `PauseExpiry`, `BreakExpiry`. New `Event`: none required — all three arrive as `EdgeTimer(now)`; the state carries what kind of edge was armed, or `ctx` reports pause. New user events: `PauseFor(now, secs)`, `CheckInYes(now)`, `CheckInNo(now)`, `BreakFor(now, secs)`, `Resume(now)`.

**State additions (keep the enum flat, no second timer):**

- `Started { checkin_at, sample_at, paused_until }` — extend today's `Started { checkin_at }`.
  - `sample_at`: next 5-min AW sample edge, `Some` only while unpaused and a task is live; `None` otherwise (this is how "zero sampling when idle/paused/break" is *structurally* guaranteed — no edge means no wake).
  - On `EdgeTimer` where `now >= sample_at`: svc has already probed AW into `ctx` (foreground app + `logged_minutes` recompute); core compares foreground ∈ task tools.
    - on-task → re-arm next `sample_at = now + sample_secs` (merged as usual), stay `Started`.
    - off-task run `< off_task_secs` → same (accumulate the off-task run counter carried in state, `off_task_since`).
    - off-task run `>= off_task_secs` → emit `ShowPrompt` interactive check-in, go to `CheckIn { kind: OffTask, .. }`.
- `Paused { resume_at, prev: Box<StartedSnapshot> }` — reachable from **any** state via `PauseFor`. Emits `HidePrompt` + arms exactly one `PauseExpiry` edge = `resume_at` (schedule edge deliberately *not* merged, so nothing fires mid-pause; on resume we recompute schedule fresh). On `EdgeTimer >= resume_at` or `Resume` → restore prior window state (recompute via `ctx.in_window`), re-arm normally.
- `Break { resume_at }` — like `Paused` but entered only from the §6.5 No→Take-a-break path; identical single-edge silencing. Kept distinct from `Paused` only for the outcomes log (user chose break vs. tray pause).
- `CheckIn { shown_at, kind: OnTask | OffTask }` — extend today's `CheckIn { shown_at }` with `kind`.
  - `CheckInYes` (off-task) → `Started` (Tier A: re-arm sample; Tier B adds classification screen effect).
  - `CheckInNo` (off-task) → emit `ShowTaskList { rows }` (rows from `task_window.rs`) + go to a lightweight `Choosing` presentation, still modeled as `CheckIn`-family so an ignored box auto-dismisses at the next schedule edge (logged no-answer, matching today's check-in semantics).
  - `BreakFor` from the No list → `Break`.

**Single-edge invariant preserved:** every branch ends in exactly one `arm_merged(runtime, ctx)` where `runtime` is the soonest of {sample, checkin, snooze, pause/break expiry} and `ctx.next_edge` is the schedule edge — the existing `arm_merged` already picks the min. Pause/Break are the only cases that arm a *bare* runtime edge (schedule intentionally suppressed). No code path arms two timers.

---

## 3. Data-model deltas (`tasks` table + classification storage)

Extend the existing `tasks` table (app is writer, svc read-only). New columns, all nullable/defaulted for forward-compat:

- `estimate_minutes INTEGER` (§6.3). Required for new tasks going forward; existing rows default `NULL` → treated as "no estimate" (window uses static fallback horizon 48 h until set).
- `logged_minutes INTEGER DEFAULT 0` (§6.3). Written by svc at sample/check-in/ack edges (the **one** svc write into `tasks`; document it — svc is otherwise read-only, this is the sanctioned exception, keyed by rowid, single UPDATE, no schema churn). Recomputed lazily; column is a cache of the last edge value so render doesn't re-scan AW.
- Tool lists — store as a child table `task_tools(task_id, app_name, kind)` where `kind ∈ {tool, ignore}` (ignore = task-scoped transient §6.4). Avoids CSV-in-a-cell; lets the selector and classification screen do set ops in SQL.
- Global classification — `app_classes(app_name, class)` where `class ∈ {favorite, normal, hidden, not_tool}` (§6.2). Single global table, app-owned.
- AW usage cache — `app_usage(app_name, minutes_90d, refreshed_at)` (§6.2 selector sort), refreshed on the GCal cadence (Phase 4+).

**Config (rules.toml, global):** `sample_secs` (default 300), `off_task_secs` (default 300), `checkin_after_secs` (already exists), break presets. These are global-only for P0 per NEXTSTEPS Open Decisions.

**Migration:** additive only. `ALTER TABLE tasks ADD COLUMN ...` guarded by a `PRAGMA user_version` bump; new child tables `CREATE TABLE IF NOT EXISTS`. Existing sessions.db opens clean; unset columns read as documented defaults. No destructive migration, no backfill needed (logged_minutes starts 0).

---

## 4. `task_window.rs` — signature + tables

New pure module in `crates/nudge-core/src/`. Zero OS calls, 100% unit-testable, mirrors `schedule.rs` conventions.

```
pub struct WindowCfg {          // from rules.toml Style/Window settings
    pub bands: Vec<(f32, StyleClass)>,   // logged/estimate → band, customizable (§6.9)
    pub not_started_red_before_secs: i64, // default 24h
    pub max_rows: usize,                 // 12 (§6.5)
}
pub struct Row {
    pub task_id: i64,
    pub title: String,
    pub deadline: Option<UnixTime>,
    pub logged: u32,
    pub estimate: Option<u32>,
    pub style: StyleClass,               // computed, ready to paint
}
pub enum StyleClass { NotStarted, NotStartedUrgent /*red outline*/, Band(u8) }

/// The one entry point. Selects tasks admitted by the dynamic horizon,
/// sorts soonest-deadline-first, caps to max_rows, and assigns per-row style.
pub fn display_list(tasks: &[Task], logged: &LoggedMap, now: UnixTime, cfg: &WindowCfg) -> Vec<Row>;
```

**Dynamic-deadline-window table (§6.8) — encoded exactly:**

| remaining = estimate − logged | admit if deadline within |
|---|---|
| < 3 h (180 min) | 48 h |
| ≥ 3 h | 72 h |
| ≥ 6 h | 96 h |
| ≥ 12 h | 168 h |

Remaining uses the *dynamic* `logged` (not static estimate); as logged grows the horizon tightens back toward 48 h. Task with no estimate → treat remaining as 0 → 48 h horizon. Sort key is always deadline-ascending regardless of admitting horizon.

**Row-style rules (§6.9):** `logged/estimate` → band via `cfg.bands`. Not-started (logged 0): `NotStarted` (white bg/black outline) unless `deadline − now < not_started_red_before_secs` → `NotStartedUrgent` (red outline).

**Shared consumers:** (a) svc §6.5 popup render routine (`overlay.rs` new `render_task_list`), (b) app `TaskListPanel` (Planner sidebar + check-in list). Both consume `Vec<Row>` verbatim — no sorting/filtering/styling in either. This is the load-bearing modularity from §6.9.

---

## 5. svc vs. app responsibility split

| Feature | Writer | Reader / actor | Reload boundary |
|---|---|---|---|
| `tasks` rows, tools, estimate, classes | **app** | svc reads read-only | app writes → fires reload event → svc re-derives |
| `logged_minutes` cache | **svc** (sanctioned exception, §3) | app reads for progress bar | no reload; svc UPDATEs by rowid at edges |
| AW sampling / mode pick / presence | **svc** (edge-only AW read) | — | none |
| `task_window::display_list` | shared core | svc popup + app panel | pure, no IPC |
| Pause (tray) | **svc** owns state | tray menu triggers `PauseFor` | none (in-process) |
| Break (§6.5) | **svc** state | check-in No path | none |
| Tool selector, classification screen, Favorites/Hidden, Not-Tools | **app** | svc consumes tool lists at sample compare | app writes → reload |
| GCal refresh, calendar auto-pause | **app** (network) | svc never touches Google | app may signal pause via a local file/event (deferred) |

Rule: svc reads everything except `logged_minutes` which it alone writes; all user-facing task/tool editing is app-side; the only IPC remains the two named events (quit, reload).

---

## 6. Ordered phases

Each phase: independently buildable/testable, ≤1 NEXTSTEPS step, concrete verification without a real 30-min wait (inject `now`, use short test intervals in core unit tests; svc integration uses config with `sample_secs=2`).

**Phase 1 — `task_window.rs` pure module (§6.8/§6.9).**
- Goal: the shared selection/sort/window/style function, fully unit-tested, no consumers yet.
- Files: `crates/nudge-core/src/task_window.rs` (new), `lib.rs` (export), `COMPONENTS.md` (register).
- State/data deltas: none (reads `Task` + a `LoggedMap`).
- Verification: table-driven unit tests hitting every §6.8 band boundary (179/180 min, 6 h, 12 h), deadline-ascending sort across admitting horizons, `NotStartedUrgent` flips exactly at `not_started_red_before_secs`, `max_rows` cap. Injected `now`.
- Risk: none to budget (pure). Only risk is spec drift — mirror the §6.8 table verbatim in a test.

**Phase 2 — data-model + migration.**
- Goal: schema for estimate, logged_minutes, task_tools, app_classes, app_usage; additive migration.
- Files: `persist.rs` (svc), app db writer, `tasks.rs` (add `estimate_minutes`, tool accessors to `Task`), `COMPONENTS.md`.
- Deltas: §3 columns + child tables; `user_version` bump.
- Verification: open an old sessions.db fixture → migration runs → old rows read with documented defaults; round-trip a task with tools/estimate; `logged_minutes` UPDATE-by-rowid path unit-tested.
- Risk: `logged_minutes` is the svc write exception — assert it's the *only* svc→tasks write in a code-review checklist item.

**Phase 3 — STARTED sampling edge + lazy logged_minutes (§6.1/§6.3).**
- Goal: the sampling spine — arm/re-arm `Sample` only in `Started`-unpaused; svc probes AW at the edge; core accumulates off-task run; `logged_minutes` recomputed lazily.
- Files: `state.rs` (extend `Started`, add `sample_at`/`off_task_since`, `Sample` handling), `lib.rs` (`EdgeKind::Sample`), svc `main.rs`/`timers.rs` (arm), `aw_query.rs` (foreground app + on-task compare), `persist.rs` (write logged_minutes).
- Deltas: §2 `Started` fields; single-edge merge unchanged.
- Verification: core unit test — Started arms `Sample` at `now+sample_secs`; an on-task sample re-arms and stays; N consecutive off-task samples cross `off_task_secs` → emits check-in ShowPrompt. Prove **no** sample edge is armed in Idle/Paused (assert `sample_at == None`). svc integration with `sample_secs=2`, AW stubbed.
- Risk: **this is the zero-polling flashpoint.** Sampling must be an armed absolute edge merged into the single timer, never a background tick. Compliant guard: `sample_at` is `None` outside Started-unpaused, so the timer literally has no sample edge to fire. Assert in tests.

**Phase 4 — OFF-task check-in Yes/No + task list + Take-a-break + Pause (§6.5/§6.6).**
- Goal: interactive check-in; No→`task_window` list + break; tray Pause with single expiry edge.
- Files: `state.rs` (`CheckIn.kind`, `Paused`, `Break`, `PauseFor`/`CheckInYes`/`CheckInNo`/`BreakFor`), `lib.rs` (`PauseExpiry`/`BreakExpiry`), `overlay.rs` (interactive box + `render_task_list` consuming `display_list`), `tray.rs` (Pause submenu), `persist.rs` (log pause/break/check-in outcomes).
- Deltas: §2 pause/break/check-in states; §4 consumer wired.
- Verification: core tests — off-task threshold → CheckIn(OffTask); Yes→Started(re-arm sample); No→ShowTaskList + auto-dismiss at next schedule edge; PauseFor from any state → single `PauseExpiry`, schedule suppressed, resume recomputes. svc integration: tray Pause silences a live prompt; overlay renders the exact `display_list` rows.
- Risk: Pause must suppress *all* edges to one expiry — verify no schedule edge leaks (test asserts the only armed edge during Paused is `PauseExpiry`). Interactive box must not steal keyboard focus (svc window-style check, `WS_EX_NOACTIVATE` retained).

**Deferred (Tier B/C, separate NEXTSTEPS steps later):** ON-task check-in (§6.4), tool classification screen, rich Tools selector (§6.2), progress-fill rendering, calendar auto-pause + GCal cap (§6.6/§6.7), launch-tools-on-select.

---

## 7. Budget-risk callouts

- **Sampling vs. zero-polling (§6.1):** risk of a 5-min background tick. Compliant form: `sample_at` is a merged absolute edge that exists *only* in Started-unpaused; structurally `None` elsewhere → no wake. Never a `set_interval`. (Phase 3 test asserts.)
- **`logged_minutes` live-ticking:** risk of a per-second counter for the progress bar. Compliant: computed lazily at sample/check-in/ack/render edges only; column is a cache. No timer for display.
- **svc writing `tasks`:** breaks the app-writer/svc-reader contract. Compliant: single sanctioned exception (`logged_minutes` UPDATE by rowid), documented in COMPONENTS.md Connection Contract; no other svc→tasks write allowed.
- **Pause/Break double-timer:** risk of arming pause-expiry *and* schedule. Compliant: pause/break arm a bare runtime edge; schedule deliberately suppressed and recomputed on resume — still one timer.
- **AW read at sample edge:** must be the existing edge-only blocking read with short timeout (AW down = treat as no-data → default on-task, no check-in escalation), never a persistent connection. No network in svc beyond localhost AW (already permitted).
- **RAM:** child tables + `Vec<Row>` are small, bounded by `max_rows=12`; no unbounded caches. `app_usage` capped to known apps. Stays well under 30 MB.

---

## Smallest shippable slice (recommendation)

Ship **Phases 1→2→3→4** as the Step-1 slice: `task_window.rs` + schema + the STARTED sampling edge + the OFF-task check-in/Pause path. That is the minimum that lets the user run a STARTED task, be detected drifting off-task, get the right-now task list brought to them, and pause honestly — directly serving the README critical goal — while every new edge folds into the existing single timer and no sampling exists outside Started-unpaused. Defer ON-task check-in, the tool-classification UI, the rich Tools selector, progress bars, and all Google/calendar pieces to later steps.

**First NEXTSTEPS step to tackle:** Phase 1 — introduce `crates/nudge-core/src/task_window.rs` (pure `display_list` with the §6.8 horizon table and §6.9 style rules) with exhaustive unit tests. It is zero-risk, blocks every consumer, and is the module the spec explicitly says must exist and does not yet.

Stop here — awaiting approval before coding.
