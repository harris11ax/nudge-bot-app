<!-- Boundary: Tier-B (NEXTSTEPS Step 3) implementation plan only — §6.4 on-task check-in,
     classification screen, §6.2 Tools selector + Settings tabs, plus carried Tier-C row-switch
     and §6.6 pause submenu. Resource budget: planning doc, no runtime cost. Execution splits
     one phase per session per NEXTSTEPS session model. -->

# PLAN-step3.md — Tier B: on-task check-in, tool classification, rich Tools selector

Planning run for NEXTSTEPS **Step 3** (Opus-gated). Scope from UI-PLAN §6.2 / §6.4, plus two
carried riders NEXTSTEPS Step 3 folds in: Tier-C row-click task switch and the §6.6 tray Pause
duration submenu. Companion to PLAN-step1.md; same phase discipline (one phase per session,
tests green before the next).

## 0. Problem / Success / Load-bearing constraints
1. **Problem:** Step 1 shipped the OFF-task drift check-in (§6.5) and left three §6.4/§6.2 pieces
   deferred: the ON-task periodic check-in, the per-tool classification screen both check-ins feed
   into, and the rich Tools selector + Settings classification UI in nudge-app.
2. **Success:** every 30 min a nominally-in-progress task raises "What are you working on?" with a
   deadline-window task list; picking a task (or answering Yes to a §6.5 drift check-in) opens a
   classification screen for the tools seen since the last check-in, each routed to the task's tool
   list / global not-tool list / task-scoped ignore list; nudge-app's new-task form and Settings
   drive `app_classes`/`task_tools` through a searchable chip selector. All wired end-to-end and
   driven live once; workspace tests green.
3. **Load-bearing constraints:**
   - Single-armed-timer invariant (PLAN-step1 §6.1): the on-task check-in cadence is a *third*
     runtime edge that must collapse into `arm_started`'s min-merge, not a new independent timer.
   - `CheckInKind::OnTask` is the variant P4 left as dead code (state.rs:23-34) — this is where it
     goes live. No new state variant should be needed; `OnTask` reuses `State::CheckIn`.
   - Classification needs **the set of tools used since the last check-in**. `aw_query::probe`
     currently returns only the *latest* window event (aw_query.rs:100-105). This set does not
     exist yet — sourcing it is a real data gap, see P1.
   - DB model is already complete: `app_classes(app_name, class∈favorite|normal|hidden|not_tool)`,
     `task_tools(task_id, app_name, kind∈tool|ignore)`, `app_usage(app_name, minutes_90d,
     refreshed_at)` with `set_app_class`/`upsert_app_usage`/`set_task_tools` writers (db.rs:77-171,
     519-565). No migration needed for classification; Tier B is wiring, not schema.
   - Core stays presentation-free (§6.9): all selection/sort/window logic in `task_window.rs`;
     svc renders, nudge-app renders, neither decides.
   - Commits/pushes remain explicit-permission (NEXTSTEPS GitHub Workflow) — write to disk, report,
     don't push unasked.

## 1. Design decisions — LOCKED (user, session 47)
- **Since-last-checkin sourcing: option A** (accumulate sampled apps in svc scratch state, reset on
  check-in). Confirmed.
- **On-task cadence is a floor and user-configurable:** `[escalation] ontask_checkin_secs` is a
  real user-facing setting (Settings, P3), default 1800. For testing it may be set to seconds to
  verify operation (mirrors Step 2b's `sample_secs`/`off_task_secs` scratch method).
- **Ordering: don't care** — optimise for reaching a functional state fastest. P1→P2 (core+svc)
  gives a working on-task check-in + classification without the frontend; P3 frontend follows.

### 1a. Sourcing detail (option A, retained for reference)

**How is "tools used since the last check-in" sourced?**

- **(A) Accumulate in svc state.** At each sample edge, push the sampled foreground app into a
  small `Vec<String>`/set carried on the svc side (not core — it's runtime scratch, not schedule
  state). Reset on any check-in resolution. Cheap (one string per 5-min sample, dedup on insert),
  no new AW calls, honours the zero-extra-wakeups budget. **Downside:** only captures apps seen at
  sample instants, so a tool used briefly between samples is missed.
- **(B) Range-query AW's window bucket** for the interval `[last_checkin, now]` at check-in time.
  Complete, but adds a new AW query shape (`events?start=…&end=…`) and one heavier read at each
  check-in.

**Recommendation: (A)**, with the accumulator living in `nudge-svc` alongside the existing sample
plumbing. Sampling is already the §6.1-sanctioned cadence; reusing its reads keeps the budget
promise and needs no new AW surface. Revisit (B) only if missed-between-samples proves to matter
in live use. **This is the load-bearing choice — confirm before P2.**

## 2. Phases

### P1 — core: `OnTask` check-in trigger + since-last-checkin accumulator plumbing — ✅ DONE session 48 (133 tests green; `OnTask` was absent, added per A.2's VERIFY note; TaskList renders via Yes/No/Break stub per A.5)
- `state.rs`: give `Started` an on-task check-in cadence edge. Reuse the existing `checkin_at`
  field's machinery is wrong (that's the periodic post-ack one) — add `ontask_at: Option<UnixTime>`
  to `Started`, armed from a new `[escalation] ontask_checkin_secs` (default 1800, 0 = off). In the
  sample-edge arm, when `ontask_at` comes due AND `ctx.foreground_on_task` is false relative to the
  *union of all due-≤window tasks' tool lists* (not just the active task), raise
  `CheckInKind::OnTask`. `arm_started` extended to min-merge the third edge so the single-timer
  invariant holds.
- `ScheduleCtx`: add `any_task_on_task: bool` (foreground app ∈ tool list of ANY task in the
  dynamic window) — computed by svc, mirrors the existing `foreground_on_task` but broadened. Core
  stays activity-blind; it only reads the boolean.
- `rules.rs`: add `ontask_checkin_secs` with default + non-negative validation (mirror
  `pause_secs`/`break_secs` at rules.rs:73-91,190).
- No `State` variant added — `OnTask` renders through `State::CheckIn { kind: OnTask }`. Extend
  `show_checkin` (state.rs:674-681) so `OnTask` gets its button set (task-list picker, not
  Yes/No/Break).
- Tests: `OnTask` fires at cadence only when off every task's tools; collapses correctly with
  sample + periodic edges; disabled at `ontask_checkin_secs = 0`. Target: keep the 122 green + new.
- **Deliverable:** core fires the on-task check-in; classification is still a no-op passthrough.
- **Not committed** until user asks.

### P2 — svc: classification screen (the biggest new piece)
- New `State`-adjacent flow: after a task is picked at a check-in (both §6.4 OnTask task-pick and
  §6.5 OffTask Yes), show the classification overlay listing each accumulated tool with three
  choices — **add to task tool list** / **global not-tool** / **task-scoped ignore**.
- New `Effect::ShowClassify { tools: Vec<String>, task_id: i64 }` + new `Event::Classify {
  app_name, choice, task_id }` (or a batched `ClassifyBatch`). Core routes; svc persists via the
  existing `set_task_tools`/`set_app_class` writers (db.rs). Ignore = `task_tools.kind='ignore'`;
  not-tool = `app_classes.class='not_tool'`; tool = `task_tools.kind='tool'`.
- svc render routine parallel to `tasklist.rs` — a new `classify.rs` overlay painting one row per
  tool with three hit-test buttons. Reuse the PrintWindow-verified layered-strip pattern from Step
  2b (NEXTSTEPS notes) — no new GDI paint path invented.
- Wire the accumulator (P1 §1 option A) reset on classify completion.
- Tests + one live drive (scratch-config method from Step 2b: `ctrl+alt+shift+Y` hotkey, +30s timer
  tolerance, PrintWindow capture).
- **Deliverable:** answering a check-in routes each tool to its bucket, persisted and visible in
  `task_tools`/`app_classes`.

### P3 — nudge-app: rich Tools selector (§6.2) + Settings Tools/Style tabs
- **New-task form selector (Planner/Triggers):** searchable multi-select dropdown → removable
  chips. Populate from `app_usage` (usage-sorted desc), favorites pinned, hidden excluded behind a
  "show hidden" toggle, substring filter, "Add tool manually" row. New ipc commands:
  `list_apps_for_selector` (joins `app_usage` + `app_classes`), reuse `set_task_tools`.
- **Settings → Tools tab:** three-way favorite/normal/hidden per app + separate Not-Tools list
  (seeded by recommendation: high `app_usage` never in any `task_tools`), editable. Read-only view
  of per-task ignore lists. ipc: `set_app_class`, `list_app_classes`, `list_not_tool_candidates`.
- **Settings → Style tab (§6.9):** band colors for `logged/estimate` completion — writes to `meta`
  or a new `style` kv; `task_window.rs` style classes already exist, this only recolors.
- svc-side `app_usage` refresh piggybacks the 24h calendar cadence (§6.7) — confirm the refresh
  hook exists in connectors.rs or add it.
- **Deliverable:** a task's tools are pickable in-app; classification decisions round-trip.

### P4 — carried riders (Tier-C row-switch + §6.6 pause submenu)
- **Row-click task switch:** today `Choosing`'s row pick ignores the row's `task_id`
  (state.rs:531-560, NEXTSTEPS Tier C). Make picking a row switch the live window/`Started` task to
  that `task_id` (launch its tools per §6.5 if not running — ShellExecute, skip running exes).
- **Tray Pause submenu (§6.6):** replace the single `Pause` item (tray.rs:37) with a hover submenu
  20m/30m/45m/1h/1.5h/Custom; each emits `Pause(secs)`. Core's `Paused` already carries `resume_at`
  — only the `Event`/`TrayCmd` gains a duration. Tray icon reflects paused state.
- **Deliverable:** row pick redirects focus; pause offers durations.

### P5 — verification
- `cargo test` workspace green; `cargo build -p nudge-svc -p nudge-ctl` clean.
- Live end-to-end drive of the on-task check-in → classification → persisted buckets, via the Step
  2b scratch-config method (document any new hotkey/timer caveats in HISTORY).
- Spot-check nudge-app selector + Settings tabs against `app_classes`/`task_tools` rows.
- Consider a subagent code-review pass (requesting-code-review skill) before declaring done.
- Log the session to HISTORY.md via nextsteps-classifier; tick Step 3 in NEXTSTEPS.

## 3. Resolved (was open) — see §1 LOCKED. No blockers remain; execute P1→P5.

---

# Appendix A — P1 detailed spec (core, `nudge-core`)

Reference line numbers are from session-47 reads of `crates/nudge-core/src/state.rs`,
`lib.rs`, `rules.rs`; verify before editing (code may have shifted).

## A.1 Data-model change — `Started` gains a fourth runtime field
`state.rs` `enum State` `Started` (currently `checkin_at, sample_at, off_task_since`, ~L67) adds:
```rust
/// §6.4 on-task check-in cadence edge. `Some(t)` = next floor tick at which,
/// if the foreground app is off EVERY due-window task's tools
/// (`!ctx.any_task_on_task`), we raise `CheckInKind::OnTask`. `None` when the
/// cadence is disabled (`ontask_secs == None`) or the task isn't live. This is
/// the THIRD runtime edge; it collapses into `arm_started`'s min-merge so the
/// single-armed-timer invariant (PLAN-step1 §6.1) still holds structurally.
ontask_at: Option<UnixTime>,
```
Threading rule per construction site (all `State::Started { … }` builds):
- **Fresh-start paths** (Ack→Started L372-390; resume-was_started L787-793; CheckIn/Choosing Yes
  L525-545): `ontask_at = ctx.ontask_secs.map(|s| now + s)` — arm the first tick one cadence out.
- **Skip path** (L411-427) and any "stop watching" path: `ontask_at = None` (Skip means don't nag).
- **Re-arm within the EdgeTimer arm** (L434-518): carry/bump per A.3.
- **Test sites** (all under `#[cfg(test)]`, ~L1150+): `ontask_at: None` unless the test targets
  on-task. Prefer routing through a builder — add `ontask_at` param to the `started(..)` helper
  (~L899) or a new `started_ontask(..)` builder to keep test churn contained.

## A.2 `EdgeKind` + `CheckInKind` + `arm_started` signature
- `lib.rs enum EdgeKind` (~L20): add variant after `Sample`:
  ```rust
  /// A §6.4 on-task check-in tick fell due: if off every due-window task's
  /// tools, ask "what are you working on?"; else silently re-arm next tick.
  OnTaskCheckIn,
  ```
- `state.rs enum CheckInKind` (~L27): the `OnTask` variant is ALREADY declared as dead code in
  P4's scaffold? — VERIFY. Current file shows only `Periodic` + `OffTask` (L28-34). If `OnTask` is
  absent, ADD it here; the NEXTSTEPS/PLAN "dead variant" note referred to the intent, not
  necessarily a committed variant. Add:
  ```rust
  /// §6.4 on-task check-in: floor-cadence "what are you working on?" raised when
  /// the user has drifted off every due-window task's tools. Answered by picking
  /// a task (→ classification, P2), not Yes/No.
  OnTask,
  ```
- `fn arm_started` (L819): extend to 3 runtime edges. Change signature to
  `arm_started(checkin_at, sample_at, ontask_at, ctx, fx)` and replace the 2-way `earliest` with a
  fold over three: `earliest(earliest(a,b), c)`. Update ALL 7 call sites (L381,452,477,497,510,539,
  790) to pass the new arg.

## A.3 Transition logic — the on-task block in the `Started` EdgeTimer arm
Insert a new block in the `(State::Started {…}, EdgeTimer|RulesReloaded)` arm (L434-518), placed
**after** the check-in block (L436-467) and **before** the sample block (L469-508), so the three
due-edge checks read top-to-bottom checkin → ontask → sample, each with an early return that
bumps its siblings via `bump_if_due` (mirroring the existing sample/checkin pattern):
```rust
// --- on-task check-in tick (§6.4): drifted off *every* due task's tools? ---
if let Some(ot) = ontask_at {
    if now >= ot {
        let next_ot = ctx.ontask_secs.map(|s| now + s);
        if !ctx.any_task_on_task {
            show_checkin(CheckInKind::OnTask, ctx, &mut fx);
            fx.push(Effect::LogEdge { entered: "checkin", at: now, mode: Some(Mode::OffTask) });
            arm_schedule(ctx, &mut fx);
            return (State::CheckIn { shown_at: now, kind: CheckInKind::OnTask }, fx);
        }
        // On every task's tools at the tick → floor says stay quiet; re-arm next
        // tick and keep the sampling spine, bumping a co-due sample edge.
        let sample_at = bump_if_due(sample_at, now, ctx.sample_secs);
        arm_started(checkin_at, sample_at, next_ot, ctx, &mut fx);
        return (State::Started { checkin_at, sample_at, off_task_since, ontask_at: next_ot }, fx);
    }
}
```
Then in the existing **sample** and **checkin** blocks' returns, add `ontask_at` (bumped if co-due:
`bump_if_due(ontask_at, now, ctx.ontask_secs)`) to every `arm_started(...)` call and every
`State::Started { … }` they build. The "nothing due" tail (L510-518) carries `ontask_at` unchanged.

**Floor semantics check:** OnTask fires ONLY at a tick boundary AND only when `!any_task_on_task`.
On-task at a tick → no prompt, next tick armed. This is the "floor, not hard cadence" the user
confirmed: 30 min is the *minimum* spacing, and a tick that lands while on-task is silently skipped.

## A.4 `show_checkin` button set (state.rs ~L674-684)
Add the `OnTask` arm to the `buttons: match kind { … }`:
```rust
CheckInKind::OnTask => Buttons::TaskList,   // new Buttons variant, A.5
```

## A.5 `Buttons` enum (state.rs ~L204-213) — new variant
Add `TaskList` (the §6.4 "What are you working on?" picker — a scrollable task list, no Yes/No):
```rust
/// §6.4 on-task check-in: pick a task from the due-window list (or "New task…").
/// Resolution routes into the classification screen (P2), not a binary answer.
TaskList,
```
For P1, `TaskList` may render as a stand-in (reuse the §6.5 list rows in `ctx.task_rows`); the real
picker + "New task…" abbreviated form is P2/P3 work. P1's job is that the edge fires and the state
is reached — note this as an intentional P1 stub so P5 doesn't flag it as a bug.

## A.6 `ScheduleCtx` + `rules.rs`
- `ScheduleCtx` (~L244): add
  ```rust
  /// §6.4: was the foreground app in the tool list of ANY task in the dynamic
  /// deadline window (not just the live task)? Svc does the union + set compare;
  /// core only branches. `true` when AW is down / nothing configured, so a
  /// missing signal never manufactures an on-task nag.
  pub any_task_on_task: bool,
  /// §6.4 on-task check-in floor cadence in seconds; `None` disables it (no
  /// `ontask_at` is ever armed). User-configurable (`[escalation]
  /// ontask_checkin_secs`, Settings, P3); set to seconds in scratch config to test.
  pub ontask_secs: Option<i64>,
  ```
  Update the test `ctx(..)` builder (~L865) and `sampling_ctx` to default `any_task_on_task: true`,
  `ontask_secs: None`.
- `rules.rs Escalation` (~L73-91): add `pub ontask_checkin_secs: i64` (default `30 * 60`), and to
  validation (~L190) `|| esc.ontask_checkin_secs < 0`. The svc maps `0 → None` for `ontask_secs`
  (mirror how `sample_secs = 0` disables sampling), so a user/test can set it to a small positive
  number of seconds to exercise the path fast.

## A.7 svc wiring (nudge-svc) — the parts P1 must stub or set so it compiles + runs
- Build `ScheduleCtx.any_task_on_task`: union the tool lists (`task_tools.kind='tool'`) of every
  task in the dynamic window (`task_window::display_list`), compare the AW foreground app against
  it. Fallback `true` when AW down / union empty (mirror `foreground_on_task`, main.rs sample path).
- Map `rules.escalation.ontask_checkin_secs` → `ctx.ontask_secs` (`0 ⇒ None`).
- **Accumulator (option A, §1):** add an svc-side `BTreeSet<String>`/`Vec` that each sample edge
  pushes the foreground app into (dedup); this is the "tools since last check-in" set P2 consumes.
  Reset it whenever a check-in resolves. P1 only needs to START accumulating; P2 reads it. Keep it
  in the svc runtime struct, NOT in core (it's scratch, not schedule state).

## A.8 Test list (P1 acceptance — extend the 122)
1. `ontask_at` armed one cadence out on Ack when `ontask_secs = Some`.
2. Not armed when `ontask_secs = None` (`ontask_at` stays `None` through Started).
3. Tick due + `!any_task_on_task` ⇒ `CheckIn{OnTask}`, `LogEdge "checkin"`, schedule armed.
4. Tick due + `any_task_on_task` ⇒ stays `Started`, `ontask_at` bumped +cadence, no prompt (floor).
5. On-task tick co-due with sample ⇒ both bumped, single armed edge = earliest of the three.
6. `arm_started` picks the earliest of checkin/sample/ontask vs the schedule edge.
7. Skip clears `ontask_at` to `None` (no on-task nag after skip).
8. Resume-was_started re-arms `ontask_at`.
Target: 122 + ~8 green; `cargo test -p nudge-core` and full workspace clean.

## A.9 P1 boundary (what P1 does NOT do)
- No classification screen (P2). `Buttons::TaskList` may reuse §6.5 rows as a stub.
- No "New task…" abbreviated form (P3). No frontend. No accumulator *consumption* (P2).
- Not committed; report readiness per NEXTSTEPS GitHub Workflow.

---

# Appendix B — P2 detailed spec (svc classification overlay + task-picker resolution)

Line refs from session-47 reads of `overlay.rs`, `tasklist.rs`, `timers.rs`, `main.rs`.

## B.0 The click→event seam (map this before touching anything)
User input flows: window `WM_LBUTTONDOWN` hit-test → `send_click(PromptClick::X)` onto a global
channel → `timers.rs` message loop drains via `poll_click()` (L141-152) → maps each `PromptClick`
to a `LoopSignal::Core(Event::…)` → `main.rs` feeds it to `next()`. Both the strip (`overlay.rs`)
and the §6.5 list (`tasklist.rs` `send_click`) share ONE channel — the classification overlay joins
the same seam. **All new clicks must extend `PromptClick` (overlay.rs ~L99-110) AND the match in
`timers.rs` L142-149.** No second input path.

## B.1 Two entry points into classification
Classification is reached after a task is affirmed at a check-in:
1. **§6.4 OnTask** — user picks a task from the `Buttons::TaskList` picker (P1 stub → real here).
2. **§6.5 OffTask Yes** — currently `CheckInYes → Outcome::CheckedIn → Started` (state.rs L525-545).
   UI-PLAN §6.5 says Yes → classification screen for tools since last check-in. So the Yes arm must
   now branch into classification instead of straight to `Started`.

Design: a new intermediate state `State::Classifying { task_id, shown_at }`. Both entry points
transition into it; it resolves (all tools classified / dismissed) into `Started` with the sampling
spine + accumulator reset.

## B.2 Core changes (`nudge-core`)
- `enum State`: add `Classifying { task_id: i64, shown_at: UnixTime }`. Add label `"classifying"`,
  `task_id()` returns `None` (not a click-through target). It's check-in-family: an ignored screen
  dies at the next schedule edge (mirror `Choosing`, state.rs L576-580).
- `enum Effect`: add
  ```rust
  ShowClassify { tools: Vec<String>, task_id: i64 },
  HideClassify,
  ```
- `enum Event`: add
  ```rust
  /// One tool's classification choice at the §6.4/§6.5 screen. Batched by the
  /// svc into N events, or a terminal `ClassifyDone` when the list is emptied.
  Classify { at: UnixTime, app_name: String, choice: ClassifyChoice, task_id: i64 },
  ClassifyDone(UnixTime),
  ```
  with `pub enum ClassifyChoice { Tool, NotTool, Ignore }`.
- `Buttons`: `TaskList` (P1) + the picker's row clicks reuse `ctx.task_rows`. The task-pick that
  enters classification needs the row's `task_id` — so unlike §6.5's `PromptClick::Start` (which
  drops the id), the OnTask picker must carry it. Add `PromptClick::PickTask(i64)` (B.4) →
  `Event::PickTask(at, task_id)`.
- Transitions:
  - `(CheckIn{OnTask}, PickTask(id))` → emit `HidePrompt`, `ShowClassify{ tools: ctx.classify_tools,
    task_id: id }`, log; → `Classifying{ task_id: id, shown_at: now }`.
  - `(CheckIn{OffTask}, CheckInYes)` → **change** current straight-to-Started (L525) to route through
    classification: `ShowClassify{ ctx.classify_tools, task_id: ctx.active_task_id }` →
    `Classifying{…}`. (Keep the Periodic-checkin Ack path going straight to Started — Periodic has
    no tool set to classify.)
  - `(Classifying, Classify{…})` → svc persists (side effect in run_effects, not core); core stays
    in `Classifying`, re-arm schedule. Core does NOT hold the tool list; each `Classify` is applied
    by the svc writer, so core just acknowledges and waits for `ClassifyDone`.
  - `(Classifying, ClassifyDone | Skip)` → `HideClassify`, `LogOutcome{CheckedIn}`, spin sampling
    spine + reset accumulator, → `Started{ …, ontask_at re-armed }`.
  - `(Classifying, BreakFor)` / schedule edge → same quiet/timeout handling as `Choosing`.
- `ScheduleCtx`: add `pub classify_tools: Vec<String>` (the accumulator snapshot, empty everywhere
  except when entering classification) and `pub active_task_id: i64` (the live task backing an
  OffTask check-in). Core copies, never computes.

## B.3 svc: `classify.rs` overlay (parallels `tasklist.rs`)
- New module `crates/nudge-svc/src/classify.rs`. Copy `tasklist.rs`'s skeleton verbatim: a
  `WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_NOACTIVATE` `WS_POPUP` tool window, `OnceLock<Mutex<…>>`
  row store, `create()/Drop`, `class_proc` with `WM_PAINT` + `WM_LBUTTONDOWN`, shared `row_rect`
  layout truth between paint and hit-test (the invariant that made Step 2b's buttons work).
- Each row = one tool name + three hit-test buttons `[Tool][Not a tool][Ignore]`. A click sends a
  new `PromptClick::Classify(app_name, ClassifyChoice)` (string payload — widen `PromptClick`, or
  keep an svc-side side table keyed by row index and send `PromptClick::ClassifyRow(usize, choice)`;
  prefer the index form to keep `PromptClick` `Copy`). On the last row classified, send
  `PromptClick::ClassifyDone`.
- Height grows with tool count (cap + scroll like the §6.5 list). Reuse Step 2b render/verify notes.
- `run_effects` (main.rs L121-140): add `Effect::ShowClassify => res.classify = Some(Classify::create(...))`
  and `Effect::HideClassify => drop(res.classify.take())`. Add `classify: Option<Classify>` to the
  svc resource struct next to `overlay`/`tasklist`.

## B.4 svc: apply a `Classify` choice (the persistence side effect)
- `timers.rs` L141-152: extend the `poll_click` match with the new `PromptClick` arms →
  `Event::Classify{…}` / `Event::ClassifyDone` / `Event::PickTask`.
- The persistence itself is a `run_effects`/main-loop side effect, NOT in core: on `Event::Classify`,
  the svc calls the existing db writers — `Tool` → `set_task_tools(task_id, +[(app,"tool")])`;
  `Ignore` → `set_task_tools(task_id, +[(app,"ignore")])`; `NotTool` → `set_app_class(app,
  "not_tool")` (db.rs L519/L540). These writers already exist and are tested (db.rs L642-650); P2
  only calls them. Note `set_task_tools` currently REPLACES the whole list (deletes then inserts,
  db.rs L521) — for incremental per-tool add, either add a `add_task_tool(task_id, app, kind)`
  upsert helper or read-modify-write. **Add `add_task_tool` (single-row `INSERT OR IGNORE`)** — one
  small, tested db method; cleaner than R-M-W.
- Reset the accumulator (A.7) on `ClassifyDone`.

## B.5 accumulator consumption (closes A.7's open end)
P1 built the svc-side "tools since last check-in" set. P2: when entering classification
(`ShowClassify` is about to be emitted), the svc snapshots that set into `ctx.classify_tools`. On
`ClassifyDone`, clear it. That is the full option-A lifecycle.

## B.6 P2 test list
1. `(CheckIn{OnTask}, PickTask(7))` → `Classifying{task_id:7}`, `ShowClassify` emitted.
2. `(CheckIn{OffTask}, CheckInYes)` → `Classifying`, NOT straight to `Started` (regression guard on
   the changed L525 arm).
3. Periodic-checkin `Ack` still → `Started` directly (no classification).
4. `(Classifying, Classify{Tool})` → stays `Classifying`, re-arms schedule.
5. `(Classifying, ClassifyDone)` → `Started`, sampling + `ontask_at` re-armed.
6. `(Classifying, schedule edge)` → dies to `Started`/`Idle` like an ignored `Choosing`.
7. db: `add_task_tool` upserts; `NotTool` writes `app_classes.class='not_tool'`.
8. Live drive (Step 2b method): OnTask fires → picker → classify a tool/not-tool/ignore → verify
   `task_tools`/`app_classes` rows + return to `Started`.

## B.7 P2 boundary
- No frontend (P3). The picker's "New task…" abbreviated form is P3 (needs the selector); for P2 the
  picker lists existing due-window tasks only — note "New task…" as a P3 stub.
- Not committed.

---

# Appendix C — P3 detailed spec (nudge-app: Tools selector §6.2 + Settings tabs)

Line refs from session-47 reads of `crates/nudge-app/src-tauri/src/lib.rs`, `Triggers.svelte`,
`App.svelte`. Frontend is **Svelte 5** (runes: `bind:value`, `onsubmit`); tauri IPC via
`@tauri-apps/api` `invoke` in `src/lib/api.js`.

## C.0 Current surface (what exists)
- New-task form lives in `Triggers.svelte` (`submitForm`, grid at L161-174): title, type, time,
  recur, deadline, mode_override, desc. **No estimate field, no tools field yet.**
- `add_task(form: NewTaskForm)` (lib.rs L124-152) already threads `estimate_minutes` into `Task`,
  but the form doesn't collect it. Tools are NOT part of `NewTaskForm`; they persist via
  `set_task_tools` (db.rs L519) which is **not yet a tauri command**.
- Registered commands: `invoke_handler![…]` (lib.rs L580-600) — no tool/app-class commands exposed.

## C.1 New tauri commands (lib.rs + register in the handler list L580)
```rust
list_apps_for_selector() -> Vec<AppDto>   // JOIN app_usage ⟕ app_classes, usage desc,
                                          //   AppDto { name, minutes_90d, class }
set_task_tools_cmd(task_id, tools: Vec<ToolDto>)   // wraps db set_task_tools
list_task_tools_cmd(task_id) -> Vec<ToolDto>
set_app_class_cmd(app_name, class)                 // favorite|normal|hidden|not_tool
list_app_classes() -> Vec<AppDto>
list_not_tool_candidates() -> Vec<AppDto>          // high app_usage never in any task_tools
set_style_bands(bands: Vec<String>)  / get_style_bands() -> Vec<String>   // §6.9 Style tab
```
Each is a thin wrapper over existing db.rs writers/readers (all already present except the
not-tool-candidate query and style kv — add those two db methods). Add matching JS in `api.js`.

## C.2 New-task Tools selector component (`src/lib/ToolSelector.svelte`)
Reusable — used by the new-task form AND the P2 picker's "New task…" form (B.7 stub closes here).
- Props: `selected: ToolDto[]` (bindable), emits changes.
- Renders: a text input (substring filter) + dropdown list from `list_apps_for_selector`, sorted
  most→least used; **favorites pinned top**; **hidden excluded** unless a "show hidden" toggle in the
  dropdown is on; a bottom **"Add tool manually"** row (free-text exe). Selected tools render as
  removable **chips** (reuse Planner's `.chip` style, Planner.svelte L98-101).
- Wire into `Triggers.svelte submitForm`: add an **Estimate (min)** number input (bind to
  `form.estimate_minutes`) and the `<ToolSelector>`; on submit call `add_task` then
  `set_task_tools_cmd(newId, selected)`.

## C.3 Settings → Tools tab (new `src/lib/settings/ToolsTab.svelte`)
- Per-app three-way radio/segmented control **Favorite / Normal / Hidden** → `set_app_class_cmd`.
  Hidden ≠ deleted (recoverable — just filtered from selectors).
- **Not-Tools list**: seeded from `list_not_tool_candidates` (high usage, never a task tool),
  editable; setting an app not-tool = `set_app_class_cmd(app,"not_tool")`.
- Read-only view of per-task **ignore** lists (`list_task_tools_cmd` where kind='ignore').

## C.4 Settings → Style tab (new `src/lib/settings/StyleTab.svelte`) — §6.9
- Color pickers for the `logged/estimate` completion bands. `task_window::StyleClass::Band(n)`
  already drives row color; this only recolors. Persist via `set_style_bands`; svc + Planner read
  `get_style_bands` at render. (If a Settings tab shell doesn't exist yet, add a minimal
  `Settings.svelte` with Tools/Style sub-tabs and route it in `App.svelte`'s tab set L6-11.)

## C.5 app_usage refresh (§6.7) — confirm or add
The selector needs `app_usage` populated. Confirm a refresh path writes `upsert_app_usage`
(db.rs L562) on the 24h calendar cadence (check `connectors.rs`/`run_connectors`). If absent, add a
90-day AW bucket aggregation to the same cadence hook. **This is the one P3 item that may reveal a
missing backend piece — probe `connectors.rs` first thing in the P3 session.**

## C.6 P3 test/verify
- `cargo test -p nudge-app` for the new commands (db round-trips already have patterns, db.rs
  L617-666).
- Manual: create a task with tools+estimate → verify `task_tools`/`tasks.estimate_minutes` rows;
  flip an app Favorite/Hidden/Not-tool → verify `app_classes`; confirm hidden apps drop from the
  selector and favorites pin.
- `npm run build` (or the project's Svelte build) clean; app launches.

## C.7 P3 boundary
- Independent of P1/P2 (§1: order-agnostic) — P3 can run first if early UI feedback is wanted, since
  it only touches nudge-app + db, not the svc state machine.
- Not committed.

---

# Appendix D — P4 detailed spec (carried riders: Tier-C row-switch + §6.6 pause submenu)

## D.1 Row-click task switch (Tier C)
Today, picking a §6.5 list row sends `PromptClick::Start` and **drops the row's `task_id`**
(tasklist.rs L274-283 comment: "the row's id is deliberately not acted on yet"; resolves as plain
Ack → `Started`, state.rs L531-545).
- tasklist.rs: change the row click to `send_click(PromptClick::PickTask(row.task_id))` (the variant
  added in B.2 — P4 reuses it; if P2 shipped first it already exists).
- timers.rs: map `PickTask(id) → Event::PickTask(now, id)` (may already exist from P2).
- state.rs `(Choosing, PickTask(id))`: switch the live window to task `id` — build `Started` bound
  to that task, and emit a new `Effect::LaunchTools { task_id }` so the svc `ShellExecute`s the
  task's required tools that aren't already running (§6.5; skip running exes — svc checks the
  process list). Distinct from the OnTask picker (which routes to classification): the §6.5 No-list
  pick means "start THIS instead," so it goes to `Started`, optionally via classification if desired
  — **decide in the P4 session; default: straight to `Started` + `LaunchTools`.**
- svc: `Effect::LaunchTools` handler in run_effects — enumerate `task_tools(kind='tool')`, filter
  out running processes, `ShellExecute` the rest.

## D.2 Tray Pause duration submenu (§6.6)
Current tray: single `Pause` item → `TrayCmd::Pause` → `Event::PauseFor(t, rules.pause_secs)`
(tray.rs L37, main.rs L248). Replace with a hover submenu.
- tray.rs: build a `Submenu` "Pause" with items **20m / 30m / 45m / 1h / 1.5h / Custom**; each
  carries its own secs. `tray-icon`'s `Submenu`/`MenuItem` API — one item id per duration.
- `TrayCmd::Pause` → `TrayCmd::PauseFor(secs)`; `Tray::poll` (tray.rs L75) maps each submenu id to
  its secs. main.rs L248 stops stamping `rules.pause_secs` and uses the carried secs.
- Custom → prompt (a tiny input window, or reuse the app) → arbitrary secs. Core `PauseFor` already
  carries the duration and `Paused{resume_at}` already generic — **no core change** beyond the
  event carrying a chosen secs (already does).
- Tray icon reflects paused state: swap icon on enter/exit `Paused` (main.rs after `next()` — check
  `state.label()=="paused"`).

## D.3 P4 test/verify
- Core: `(Choosing, PickTask(id))` → `Started` bound to `id` + `LaunchTools{id}` emitted.
- svc: `LaunchTools` skips already-running exes (unit-test the filter with a stubbed process list).
- Live: tray Pause submenu → pick 20m → verify single `PauseExpiry` edge at +20m, icon shows paused;
  §6.5 No-list → pick a different task → that task goes live + its tools launch.

---

# Appendix E — P5 verification (unchanged from §2 P5, expanded)
- `cargo test` (workspace) + `cargo build -p nudge-svc -p nudge-ctl` clean; `nudge-app` builds.
- Full live end-to-end via the Step 2b scratch-config method (hotkey `ctrl+alt+shift+Y`, +30s timer
  tolerance, `PrintWindow` + `PW_RENDERFULLCONTENT` captures), set `ontask_checkin_secs` to a small
  seconds value: drive prompting→Start→started→**on-task check-in fires**→pick task→**classification
  screen**→classify tool/not-tool/ignore→back to Started; verify every step via `sessions.db` edges
  + `task_tools`/`app_classes` rows.
- Spot-check nudge-app selector + Settings tabs against the db.
- Subagent code-review pass (requesting-code-review skill) before declaring done.
- Log session to HISTORY.md via nextsteps-classifier; tick Step 3 in NEXTSTEPS; report commit
  readiness (don't push unasked, per GitHub Workflow).

*NEXTSTEPS Step 3 points here. Each phase is one execution session per the NEXTSTEPS session model.*
