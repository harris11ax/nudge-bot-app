# HISTORY.md — Session Log
<!-- Session-by-session history, pulled out of NEXTSTEPS.md to keep that file forward-looking only.
     Repo is live at github.com/harris11ax/nudge-bot-app (see NEXTSTEPS.md "GitHub Workflow"), but as of
     session 40 only the initial commit is actually pushed — this file is still the source of truth for
     everything since, until the user asks to commit/push and optionally migrate this log into Issues/Wiki. -->

## Verified working (sessions 2–7)
- Toolchain: Rust 1.97 + VS2022 C++ build tools. `cargo build --release` clean.
- nudge-svc live: tray (`tray-icon` crate: Toggle anchor / Reload rules / Quit — all exercised),
  anchor overlay (layered topmost strip), Ctrl+Alt+N hotkey, edges → sessions.db.
- Graceful shutdown: `Local\nudge-bot-quit` event; `nudge-ctl quit` → 0 orphans. (`Stop-Process` still orphans tray.)
- Live reload: `Local\nudge-bot-reload` event; `nudge-ctl` edit verbs (`anchor`, `nudge ls|add|rm`, `reload`)
  are format-preserving (toml_edit), validate-before-write, atomic rename, auto-signal running svc.
- Event-name literals duplicated in svc + ctl — **keep in sync** (quit + reload).
- Config at `%LOCALAPPDATA%\nudge-bot\rules.toml`. `scripts\register_tasks.ps1` written, NOT run (autostart off).

## DONE sessions 8–34
- 8–30: escalation ladder + state machine + check-in; nudge-draft LLM crate; on/off-task classification;
  quick-add parser + tasks table + Tauri shell; svc window-gen consumer + click-through launcher.
- 31–33: 9d-ii task-id click-through + 9f mode_override threading. `cargo test --workspace` clean (76).
  Once/deadline-only task firing still deferred (per-task done-flag) — blocks 10e end-to-end.
- 34 (Opus): §10 Calendar+connectors planning run → GOOGLE-PLAN.md, decomposed into 10a–10e. All
  Google network I/O in nudge-app (svc stays offline); PKCE loopback OAuth + DPAPI token cache.

## DONE session 35
- 10a — Google OAuth + token cache: `google/mod.rs` (client config, DPAPI-sealed token cache,
  `google_status`/`google_connect` commands, `access_token()` for 10b+) + `google/oauth.rs` (PKCE
  loopback per RFC 8252; S256 challenge + randomness via Windows CNG/bcrypt.dll — no rand/sha2 crate;
  token exchange/refresh via blocking `ureq`). Connect button wired into Settings tab. `cargo test --lib`
  9/9 (incl. an RFC 7636 PKCE vector check against the real BCrypt bindings), `npm run build` clean,
  workspace unaffected. No live OAuth-consent E2E run (needs a real Google Cloud client). Unblocks 10b.

## DONE session 36
- 10b — Calendar READ: `google/calendar.rs` (calendarList+events.list via blocking `ureq`; pure JSON
  parsers unit-tested incl. a hand-rolled RFC3339/civil-date parser — no chrono dep). App-only
  `calendars`/`cal_events`/`meta` tables in db.rs. `refresh_calendars` enforces §6.7's 24h cap
  (`force` param for the manual button). `Calendar.svelte`: month grid, event+deadline overlay,
  offline grey-out on refresh failure, legend; per-calendar checkboxes added to Settings tab.
  `cargo test --lib` 22/22 (up from 9), `npm run build` clean, workspace `cargo test --workspace`
  76/76 unaffected. No live GCal API smoke test (same caveat as 10a — needs a real Google Cloud
  client + consent click-through). Unblocks 10c.
- A tool-output system-reminder mid-session appeared to claim `google/mod.rs` had been edited
  outside the session. Initially treated as a possible prompt injection and the file was restored.
  Root cause since confirmed: a separate concurrent session running the same instruction set was
  legitimately editing the same file at the same time — not an injection. No action needed.

## DONE session 37
- Diagnosed+fixed nudge-app showing WebView2 "localhost refused to connect" in place of the GUI.
  Root cause (confirmed by reading `tauri-macros-2.6.3/src/context.rs:155`, not guessed): Tauri only
  loads the bundled UI when the `custom-protocol` feature is compiled in; that feature is NOT default
  and is normally set by the `tauri build` CLI. Session 36's verification ran a plain
  `cargo build --release` (matching the rest of the workspace's convention), which silently produced a
  binary that still points at the Vite dev server (`http://localhost:1420`) even in a release profile.
  Nothing was listening there → connection-refused. Unrelated to OAuth; coincidental timing. Fixed by
  rebuilding via `npm run tauri build -- --no-bundle` and relaunching; confirmed via
  `Get-NetTCPConnection` the new process makes zero outbound connection attempts. Also confirmed (not
  a task, just state): `google_client.json` / `google_tokens.json` still don't exist in
  `%LOCALAPPDATA%\nudge-bot`, so Connect Google can't actually run yet — needs the user's real Google
  Cloud OAuth client.

## DONE session 38
- Step 1 — Hardened nudge-app launch: `scripts\launch-both.ps1` now always rebuilds nudge-app via
  `npm run tauri build -- --no-bundle` (in `crates\nudge-app`) before launching, instead of running
  whatever binary was already sitting in target\release. Prevents recurrence of the
  custom-protocol/localhost:1420 dev-server bug (session 37). Verified end-to-end: ran the updated
  script, release build completed (3m16s), nudge-app.exe launched, confirmed via
  `Get-NetTCPConnection` zero outbound connections (same method as session 37) — i.e. it's serving the
  bundled UI, not the Vite dev server. nudge-svc.exe + nudge-app.exe stopped afterward to restore
  prior system state (neither was running before this verification).
- NEXTSTEPS.md trimmed: this file split out to hold session history; design/engineering details not
  captured elsewhere were migrated into COMPONENTS.md, GOOGLE-PLAN.md, and nudge-app/README.md (see
  those files' history for what moved where — not duplicated here to avoid drift).

## DONE session 39
- 10c — Calendar WRITE (primary only): `google/calendar.rs` adds `events.insert`/`events.update`
  (`create_event`/`update_event`) + a pure `build_event_body` helper (all-day → `{date}`, timed →
  `{dateTime}`, reusing the existing hand-rolled RFC3339/civil-date code — still no chrono dep).
  `google/mod.rs` SCOPES widened to add `calendar.events` alongside `calendar.readonly`; reconnect
  re-consents since `oauth.rs` always sends `prompt=consent`. `db.rs` adds `Store::upsert_event`
  (single-row `cal_events` upsert), `Store::delete_event`, and `Store::primary_calendar_id`/
  `set_primary_calendar` (`meta.primary_gcal_id`, falling back to whichever calendar Google reports
  as primary if the user hasn't picked one). New Tauri commands `primary_calendar`,
  `set_primary_calendar`, `create_event`, `update_event`. Write flow: optimistic local upsert first
  (temp id for create), then a synchronous push to Google (this crate has no async runtime, so
  "optimistic" here means write-ordering, not a non-blocking UI) — reconciled to the authoritative
  row on success; on push failure the optimistic row stays cached and the error is surfaced to the
  caller. Frontend: `api.js` wrappers; `GoogleConnect.svelte` gets a radio-button "Write target"
  picker under the existing per-calendar checkboxes; `Calendar.svelte` gets a create/edit dialog
  (click a day to create, click an event to edit — title, all-day toggle, date, start/end time), new
  overlay/dialog CSS in `app.css`, and `role=button`/`tabindex`/`onkeydown` on the day/event cells to
  stay keyboard-accessible (matches the project's existing `<button>`-first convention; `npm run
  build` shows zero a11y lint warnings). `cargo test --lib` in `nudge-app/src-tauri` 25/25 (up from
  22). `cargo test --workspace` 76/76 unaffected. `npm run build` clean. `cargo build --release`
  clean (4m07s). No live GCal API smoke test — same caveat carried from sessions 35–37:
  `google_client.json`/`google_tokens.json` still don't exist in `%LOCALAPPDATA%\nudge-bot`, so
  `events.insert`/`update` are unit-tested (JSON body shape) but never round-tripped against real
  Google infrastructure. Unblocks 10d.
- Separately (outside this session's scope, observed already applied when this session resumed after
  an interruption): the GitHub remote was linked (`origin` →
  `https://github.com/harris11ax/nudge-bot-app.git`, `main` tracking `origin/main`, verified via
  `git remote -v`/`git branch -vv`) and NEXTSTEPS.md picked up a "GitHub Best Practices" section
  (branch/commit/PR conventions). Noted here for continuity, not re-verified beyond confirming the
  remote config and branch tracking are actually correct.

## DONE session 40
- 10d — Suggested-triggers inbox: new app-only `suggested_triggers` table (`db.rs`) — `id, title,
  description, deadline, source, gcal_event_id, status, created_unix` — parallel to `tasks` but never
  read by nudge-svc, so a connector guess never becomes a live trigger without a human Accept.
  `Store::list_suggested_triggers` (pending, newest first), `Store::insert_suggested_trigger`
  (connector ingest seam, unused until 10e — expected dead-code warning), `Store::accept_suggested_trigger`
  (inserts into `tasks` carrying over title/description/deadline/source→trigger_source/gcal_event_id,
  marks the row `accepted`, returns the new task id — rejects if the row isn't `pending`), and
  `Store::dismiss_suggested_trigger` (marks `dismissed`). Tauri commands `list_suggested_triggers`,
  `accept_suggested_trigger` (also `signal_reload()`s the svc since it creates a task),
  `dismiss_suggested_trigger`. Frontend: `api.js` wrappers; `store.svelte.js` gets a
  `suggestedTriggers` array + `refreshSuggestedTriggers`/`acceptSuggestion`/`dismissSuggestion`;
  `Triggers.svelte` gets a "Suggested" section (only rendered when non-empty) above Quick add, with a
  source badge (Manual/Gmail/Calendar), title, optional deadline, and Accept/Dismiss buttons, scoped
  `<style>` block for the new list/badge classes. `cargo check`/`cargo test --lib` in
  `nudge-app/src-tauri`: 25/25 unchanged (no new Rust tests added — table is empty until 10e connectors
  populate it, so there's no non-trivial logic to unit-test beyond the accept/dismiss SQL already
  exercised implicitly via `cargo check`). `svelte-check` itself is broken in this environment
  (`TypeError: Cannot read properties of undefined (reading 'useCaseSensitiveFileNames')` — a
  toolchain/TS-version mismatch unrelated to this change); frontend changes reviewed by hand against
  existing patterns instead. Unblocks 10e (Gmail/GCal connectors write into `suggested_triggers` via
  `insert_suggested_trigger`).

## DONE session 41
- 10e — Gmail/GCal connectors: candidate ingest into the `suggested_triggers` inbox from two sources,
  deduped so nothing doubles up.
  - `google/gmail.rs` (NEW): `gmail.readonly` REST — `users.messages.list` (query + maxResults) and
    `users.messages.get` (`format=metadata`, Subject/From/Date headers only — never fetches a body).
    `EmailCandidate { message_id, subject, from, snippet, internal_date_unix }`. Pure parsers
    (`parse_message_ids`, `parse_message`, case-insensitive `header`) split from thin `ureq` network
    fns, same idiom as `calendar.rs`. `list_candidates` skips a message that fails to fetch rather
    than sinking the batch. `internalDate` (epoch ms string) → unix seconds.
  - `connectors.rs` (NEW, top-level module): `run_connectors(store, token, now)` deposits from both
    sides. GCal: reads upcoming timed events from the local `cal_events` cache (`store.list_events`,
    14-day horizon), skips all-day/past/already-known, title backfilled if blank. Gmail: live
    `list_candidates` over `in:inbox newer_than:7d` (max 25), keeps only messages matching an
    action-keyword heuristic (due/deadline/rsvp/respond/submit/… over subject+snippet, lowercased),
    strips `Re:`/`Fwd:`/`Fw:` prefixes, char-boundary-safe clamp to 120, sender kept in the
    description for provenance. Returns `ConnectorSummary { gcal_added, gmail_added, gmail_scanned }`.
    All classify/phrase helpers pure + unit-tested; Gmail (the only network dep) injected as candidates
    in tests.
  - Dedup (`db.rs`): `known_gcal_event_ids` (UNION of `tasks.gcal_event_id` and pending
    `suggested_triggers.gcal_event_id`) guards gcal candidates across both a live task and the inbox,
    and across successive runs; intra-run dup guarded by inserting into the in-memory set as we go.
    `pending_titles_for_source("gmail")` dedups Gmail by cleaned title (no stable external id in this
    schema — re-scanning the same email yields the same title and is skipped).
  - Scope: `gmail.readonly` appended to `SCOPES` (`google/mod.rs`); a user connected before 10e holds
    only the calendar scopes, so `google_connect`'s `prompt=consent` re-consent picks Gmail up on
    reconnect.
  - Wiring: `run_connectors` Tauri command (`refresh_calendars(false)` first so the cache the connector
    reads is fresh, then Gmail live) returning `ConnectorSummaryDto`; `api.js` `runConnectors`;
    `store.svelte.js` `scanConnectors` (runs + reloads inbox, returns summary); `Triggers.svelte`
    Suggested section reworked to always render with a "Scan Gmail + Calendar" button + result/empty
    copy (was only rendered when non-empty).
  - Tests: 13 new unit tests (5 gmail parsers, 8 connector heuristics incl. char-boundary clamp);
    `cargo test --lib` in `nudge-app/src-tauri` 37/37 pass. `vite build` green (svelte-check has no
    script here; frontend built instead). 
  - DEFERRED: the optional nudge-draft LLM pass to sharpen titles / infer times — nudge-draft is a
    binary crate (`main.rs`, not a lib) and wiring it in needs a lib split + API-key plumbing;
    deterministic heuristics shipped as the floor and keep the connector offline-diagnosable.
  - STILL BLOCKED end-to-end: accepted gcal/deadline suggestions land as `Recur::Once` tasks, which
    `schedule.rs` still skips (needs the deferred per-task done-flag). Connector fills the inbox and
    accept creates the task, but one-shot firing is the remaining prereq for this to nudge.

## DONE session 42 (Opus) — Step 1 / PLAN-step1 Phase 3: STARTED sampling edge + lazy logged_minutes
The §6.1 sampling spine: once a task is Started, nudge-svc samples the foreground app on a cadence and
notices when the user has drifted off-task. This is the state-machine half of the feature — the check-in
it raises still uses the existing kindless prompt; the interactive Yes/No + task-list + Pause path is P4.

- **Core (`lib.rs`, `state.rs`)**: new `EdgeKind::Sample`. `Started` widens to
  `{ checkin_at, sample_at, off_task_since }`. `sample_at` is `Some` **only** in `Started` with sampling
  configured — Idle, Prompting, a showing CheckIn, and a skipped window all carry none, so the zero-polling
  budget (PLAN §7) holds structurally: there is no edge to wake on rather than a guard that declines to
  fire. `off_task_since` anchors the current *continuous* off-task run; returning to a tool clears it, so a
  glance elsewhere never accumulates toward a nag. Run ≥ `off_task_secs` → drift check-in.
- **Single-timer invariant**: `Started` is the first state with two runtime edges. `arm_started` collapses
  check-in + sample to their earliest, then `arm_merged` pits that against the schedule edge — still exactly
  one `ArmEdgeTimer` per transition (asserted). `bump_if_due` pushes a sibling edge that came due on the
  same wake to the next interval instead of re-arming it in the past (which would spin a second wake).
- **Config (`rules.rs`, `schedule.rs`, `rules.example.toml`)**: `[escalation] sample_secs` (0 = off,
  **default off**, mirroring `checkin_after_secs`) + `off_task_secs` (default 300). Opt-in was chosen over
  the plan's "default 300" so an existing rules.toml keeps its pre-P3 behaviour and no one gets sampled
  without asking.
- **svc (`main.rs`)**: `sample_due` gate (probe AW only at a due sample edge in Started) +
  `foreground_on_task` compare — the task's own `task_tools` list wins, else global `[classify]
  productive_apps`. `ignore`-kind tools count as on-task. **Three "we can't tell" paths deliberately read as
  on-task**: AW down (no foreground), nothing configured anywhere (empty tool list *and* empty
  productive_apps — otherwise every app is drift and it nags forever), and any non-sample edge. On an
  on-task sample, one cadence folds into `logged_minutes` via `set_logged_minutes` — the sanctioned
  svc→tasks write (§3), lazy at the edge, never ticked.
- **CheckIn exit**: Ack resumes sampling with a clean run; Skip stops it (matches Skip's dismiss-for-good
  meaning). An auto-resolved presence check-in keeps the spine alive.
- **Tests**: 12 new core tests (sample arming, earliest-of-three merge, on-task quiet re-arm, threshold
  crossing, run reset, AW-down never drifts, **no Sample edge outside Started** across every state, skip,
  auto-checkin bump, Ack/Skip resume/stop, nothing-due re-arm) + 4 svc tests (sample gate, task_tools
  precedence + case-insensitivity, productive_apps fallback, unknowable→on-task). `cargo test --workspace`
  **106/106 green**. Not committed.
- **P4 note**: PLAN §2's `CheckIn { kind: OnTask | OffTask }` is *not* in yet — P3 routes the drift check-in
  through the existing kindless `CheckIn`. P4 needs the kind to split Yes→keep-going from No→`ShowTaskList`.

## DONE session 43 (Opus) — Step 1 / PLAN-step1 Phase 4: OFF-task check-in Yes/No + task list + Break + Pause
- **`CheckIn { kind }` landed** (the P4 note above): `CheckInKind::{Periodic, OffTask}`. Deviation from
  PLAN §2's `OnTask | OffTask` — the on-task check-in is §6.4, an explicit Tier-B defer, so that variant
  would be dead code; `Periodic` names the pre-existing post-ack check-in the plan's list had no name for.
  Periodic keeps Start/Skip; OffTask is the §6.5 drift question and takes Yes/No/Break.
- **New states**: `Choosing { shown_at }` (the §6.5 list is up — check-in family, so an ignored list dies at
  the next schedule edge exactly as an ignored check-in does), `Paused { resume_at, was_started }`,
  `Break { resume_at }`. New events `CheckInYes/CheckInNo/PauseFor/BreakFor/Resume`; new effects
  `ShowTaskList { rows }` / `HideTaskList`; new `EdgeKind::{PauseExpiry, BreakExpiry}`; new outcomes
  `CheckedInNo/Paused/BreakTaken/Resumed`.
- **Pause is handled *before* the `!in_window` collapse** — the load-bearing bit. A pause must outlive the
  window it started in and must arm a *bare* expiry edge with `ctx.next_edge` deliberately un-merged;
  routing it through the normal path would have let a WindowEnd fire mid-pause. Resume re-derives from the
  schedule as it stands then (nothing stale to honour, since nothing was armed). `was_started` exists so a
  pause taken mid-task resumes to `Started` rather than re-nagging the user to start what they were doing.
- **`Buttons` is core data, not overlay choice**: `ShowPrompt` now carries `Buttons::{StartSnoozeSkip,
  YesNoBreak}`; overlay packs it into `GWLP_USERDATA` bit 9 so paint and hit-test read one source.
- **`tasklist.rs` (new svc module)**: layered/topmost/NOACTIVATE centred window painting `display_list`
  rows verbatim — zero select/sort/style (§6.9). Row click → `Start` ("I'm on it"); footer → `Break`.
  Both post to overlay's existing click channel via `overlay::send_click`. Height bounded by `max_rows`.
- **`task_list_due(state)`** gates `display_list` to a live drift check-in / open list, mirroring the
  `sample_due`/`checkin_due` edge-gating discipline; `progress_map` reads `logged_minutes` off rows already
  loaded, so building the list re-reads nothing.
- **tray**: Pause / Resume (always enabled — core ignores a stray Resume). Durations from new
  `[escalation] break_secs = 600` / `pause_secs = 1800`; `timers.rs` hands up `LoopSignal::Pause/Break` and
  `main.rs` stamps the duration on, keeping rules out of the message loop.
- **Tests**: 122/122 workspace green (was 106). +10 core (drift asks Yes/No & periodic doesn't, Yes resumes
  sampling, No shows the list verbatim, ignored list dismisses at the schedule edge, Choosing resolves on
  either answer, break silences→resumes the task, **Pause from every state arms only its expiry with the
  schedule suppressed**, paused stays silent through every event incl. window close, resume recomputes
  three ways, no Sample edge while paused/on-break) + 4 tasklist + 2 svc (`task_list_due`, `progress_map`).
- **Verification honesty**: svc boots clean (AW probe OK, arms, no panic). The new *windows* — task-list
  render, Yes/No hit-test, tray Pause — are **not** driven end-to-end; that needs a live task window plus a
  real drift against the user's own `%LOCALAPPDATA%\nudge-bot\rules.toml`, which this session did not touch.
- **Deferred, deliberately**: tray Pause *submenu* of durations (single configured duration for now); a row
  click resolves the check-in but does **not** switch the live window to the picked task (PLAN Tier C,
  app-side). Not committed.

## Session 46 (2026-07-16) — Step 2b: button-paint "bug" + off-task check-in, resolved & driven live
- **Both 2b items closed with zero code changes** — each was environmental, not a defect.
- **Buttons were always painting.** Session 44's screenshots used BitBlt-family capture
  (`Graphics.CopyFromScreen`), which skips `WS_EX_LAYERED` windows without `CAPTUREBLT`. Capturing the
  `NudgeAnchor` hwnd via `PrintWindow(..., PW_RENDERFULLCONTENT)` shows face/frame/labels rendered for
  both button sets. Works even at the Windows lock screen — no computer-use permission needed.
- **Off-task check-in fires.** Two stacked masks in sessions 44/45's setup: (1) the 30s tolerable-delay
  on `SetWaitableTimerEx` stretches a 15s sample cadence to ~45s real; (2) a rules-window not linked to a
  task has `window_task_id = None`, and with `productive_apps` empty, `foreground_on_task` deliberately
  reads on-task ("nothing configured → never nag") — so no drift could ever accrue. With
  `[classify] productive_apps` set in the scratch config, the whole §6.5 path ran.
- **Driven end-to-end** (scratch `$env:LOCALAPPDATA`, 2–3s test durations + temporarily shrunk timer
  tolerance, `PostMessage` clicks, `PrintWindow` captures, sessions.db edge assertions):
  prompting→Start→started → sample edges → drift check-in `[Yes][No][Break]` → Yes→started;
  No→choosing + task list ("What's actually due", red-outline <24h row, Take-a-break) →
  break→expiry→resumed. Instrumentation reverted; 122/122 workspace tests green.
- **Still unexercised**: tray Pause/Resume (real tray-menu click required; synthetic input can't reach
  it) — fold into Tier B's live pass. Not committed.

## Session 48 (2026-07-16) — Step 3 / PLAN-step3 P1: §6.4 on-task check-in core + svc plumbing
- **Core (`nudge-core`)**: `CheckInKind::OnTask` + `Buttons::TaskList` + `EdgeKind::OnTaskCheckIn`;
  `Started` gains `ontask_at` (third runtime edge), armed on Ack/Yes/resume, cleared on Skip;
  `arm_started` 3-way min-merge keeps the single-armed-timer invariant. Tick due + `!any_task_on_task`
  → `CheckIn{OnTask}`; on-task at the tick → silent re-arm (floor semantics), co-due sample bumped.
  `ScheduleCtx` gains `any_task_on_task` (default true — no signal never nags) + `ontask_secs`.
- **Rules**: `[escalation] ontask_checkin_secs` (default 1800, 0 disables, negative rejected),
  `Escalation::ontask()`. NOTE: defaults ON — existing configs now arm a 30-min on-task tick (silent
  unless drifted off every due task's tools).
- **svc**: `ontask_due` gate + `any_task_on_task()` (union of `task_tools` across `display_list`
  due-window rows; fallback global productive_apps, else true); `Buttons::TaskList` renders as the
  Yes/No/Break stub (P1 stand-in per PLAN A.5 — real picker is P2); `task_list_due` includes
  `CheckIn{OnTask}`; §1-option-A accumulator `seen_tools: BTreeSet<String>` fills at sample/ontask
  probes, clears on check-in resolution (P2 consumes it).
- 133 workspace tests green (was 122; +9 core state, +1 rules, +1 integration adjusted, +2 svc). Not committed.

## Session 49 (2026-07-16) — Step 3 / PLAN-step3 P2: classification screen (svc + core)
- **Core (`nudge-core`)**: `State::Classifying { task_id, shown_at }` (check-in family — ignored screen
  dies at the schedule edge, break/pause silence it, `was_live` counts it); `Effect::{ShowClassify,
  HideClassify}`; `Event::{PickTask(t,id), Classify{at,app_name,choice}, ClassifyDone(t)}` +
  `ClassifyChoice::{Tool,NotTool,Ignore}`; `ScheduleCtx.classify_tools` (accumulator snapshot, empty
  outside check-in family). Entry points: §6.5 OffTask **Yes** (guarded: tools accumulated AND
  `window_task_id` present — else plain resolution, pre-P2 behaviour preserved; deviation from PLAN B.2:
  reused `window_task_id` instead of adding `active_task_id`) and §6.4 OnTask **row pick**
  (`PickTask` carries the row's id; classification routes to the *picked* task). `show_checkin(OnTask)`
  now emits `ShowTaskList` — the picker IS the task list (P1's Yes/No/Break stub retired);
  `hide_visible` tears it down from `CheckIn{OnTask}` too. `(Classifying, Classify)` stays put (svc
  persists); `ClassifyDone`/`Skip` → `Started` with sample + ontask spine re-armed; reload re-emits.
- **svc**: new `classify.rs` overlay (tasklist.rs skeleton: layered/topmost/noactivate, shared
  `row_rect`/`btn_rect` paint+hit-test truth) — one row per tool, `[Tool][Not a tool][Ignore]` buttons,
  chosen face highlight, Done footer; last-row choice auto-sends ClassifyDone. `PromptClick::{PickTask(i64),
  ClassifyRow(usize,choice), ClassifyDone}` (index form keeps it `Copy`; `classify::app_at(i)` resolves at
  drain, stale index dropped). tasklist rows now send `PickTask(row.task_id)` — `(Choosing, PickTask)`
  resolves like Start (id acted on in P4/Tier-C). Persistence at the main-loop seam on `Event::Classify`:
  new `persist.rs` writers `add_task_tool` (single-row INSERT OR IGNORE) + `set_app_class` (upsert);
  Tool/Ignore → `task_tools`, NotTool → `app_classes`. `ctx.task_rows` filled at the ontask tick (picker
  rows ready the moment the check-in fires); `ctx.classify_tools` snapshotted while check-in family;
  accumulator now clears only when leaving {CheckIn, Choosing, Classifying}.
- 143 workspace tests green (was 133; +7 core, +2 svc classify, +1 persist). `cargo build -p nudge-svc
  -p nudge-ctl` clean (2 pre-existing warnings). **Live drive deferred to P5** (PLAN E covers the same
  path end-to-end; Step-2b scratch method notes apply). Not committed.

## Session 50 (2026-07-16) — Step 3 / PLAN-step3 P3: nudge-app Tools selector + Settings Tools/Style tabs
- **db.rs (app)**: new readers `list_apps_for_selector` (union of `app_usage` ⟕ `app_classes`, usage
  desc — class-only apps still appear), `list_app_classes`, `list_not_tool_candidates(limit)` (high
  usage, never in any `task_tools`, unclassified), `list_all_ignores` (kind='ignore' joined to task
  titles) + `AppRow`. New test `selector_and_not_tool_candidate_queries`.
- **C.5 gap confirmed + closed**: no `app_usage` refresh path existed anywhere. New app-side module
  `aw_usage.rs` — 90-day AW window-bucket range read (`events?start=&end=`), per-app `duration`
  aggregation to minutes (pure, unit-tested), svc `aw_query` conventions (prefix bucket match,
  fail-to-empty; looser 20s read budget for the big body). `refresh_app_usage(force)` command applies
  the §6.7 24h cap via `meta.app_usage_last_refresh`; AW-down refreshes nothing and does NOT stamp,
  so retry isn't gated.
- **lib.rs**: 10 new commands registered — `list_apps_for_selector`, `set_task_tools_cmd` (+svc reload
  ping), `list_task_tools_cmd`, `set_app_class_cmd` (class validated), `list_app_classes`,
  `list_not_tool_candidates`, `list_task_ignores`, `get/set_style_bands` (JSON array in
  `meta.style_bands`, [] = renderer defaults), `refresh_app_usage`. `AppDto`/`ToolDto`/`IgnoreDto`.
- **Frontend**: new `ToolSelector.svelte` (§6.2: substring filter, usage-sorted, favorites pinned,
  hidden behind toggle, not-tools excluded, removable chips, "Add tool manually"); Triggers full form
  gains Estimate (min) + Tools (persists via `set_task_tools` after insert — `createTask` now returns
  the TaskDto). New `settings/Settings.svelte` shell (General/Tools/Style sub-tabs; General keeps
  GoogleConnect) replacing App.svelte's inline stub; `ToolsTab.svelte` (favorite/normal/hidden
  segmented + not-tool, Not-Tools list w/ restore, candidate seeding, read-only per-task ignores,
  manual usage-refresh button); `StyleTab.svelte` (3 band color pickers → `meta.style_bands`).
- nudge-app backend tests 43 green (was 40); `vite build` clean. svc/core untouched. Not committed.
- Deferred: renderers don't consume `style_bands` yet (svc/Planner read is follow-up); ontask cadence
  stays rules.toml-only (Settings edit of rules.toml out of app scope).

## Session 51 (2026-07-16) — Step 3 / PLAN-step3 P4: Tier-C row-click task switch + tray Pause submenu
- **Core (state.rs)**: `Started` gains `task_id: Option<i64>` — the task the sampling spine measures.
  Seeded from `ctx.window_task_id` at every fresh start (Ack, check-in resolutions, resume); `None` on
  Skip. New `Effect::LaunchTools { task_id }`. New Tier-C arm `(CheckIn|Choosing, PickTask(id))` →
  hide, `LaunchTools`, log started/CheckedIn, `Started { task_id: Some(id) }` — PickTask removed from
  the generic resolution union (a pick now *switches*, not just dismisses). The guarded OnTask-pick →
  classification arm also emits `LaunchTools`; `(Classifying, ClassifyDone|Skip)` lands `Started`
  bound to the classified task. LaunchTools deliberately NOT emitted on plain Yes/Ack (no surprise app
  launches on "still on it"). Known nit: a pause during a picked task resumes bound to the *window's*
  task (`Paused` only keeps `was_started: bool`).
- **svc**: `launch.rs` gains `launch_tools(db, task_id)` — `task_tools(kind='tool')` minus running
  exes (Toolhelp snapshot, case-insensitive; pure `tools_to_launch` filter unit-tested), rest launched
  via `ShellExecuteW` (PATH/App-Paths resolution, best-effort per exe). New Cargo feature
  `Win32_System_Diagnostics_ToolHelp`. `run_effects` handles `LaunchTools`. Sample edge now compares/
  accrues against the `Started`-carried task (fallback `window_task_id`), so a switch redirects
  `logged_minutes` too.
- **Tray (§6.6)**: single Pause item → `Submenu` 20m/30m/45m/1h/1.5h + "Default (rules)"
  (`PAUSE_CHOICES`); `TrayCmd::Pause(Option<i64>)` / `LoopSignal::Pause(t, secs)`, main stamps
  `pause_secs` only on the None fallback. Custom free-input duration deferred (needs an input surface;
  Settings UI candidate). Tray icon now mirrors paused state — grey pause-bars tile vs blue arrow
  (`set_paused`, flipped only on the Paused-state edge).
- 145 workspace tests green (was 143; +2 core, +1 svc launch filter, net of reworked pick tests).
  `cargo build -p nudge-svc -p nudge-ctl` clean. Live drive (incl. tray submenu real-click) deferred
  to P5 per plan. Not committed.

## Session 52 (2026-07-16) — Step 3 / PLAN-step3 P5: verification — Step 3 DONE
- 145 workspace tests green; `cargo build -p nudge-svc -p nudge-ctl` clean; nudge-app `vite build` clean.
- **Live end-to-end drive** (Step 2b scratch-config method, `ontask_checkin_secs=40`/`sample_secs=10`,
  task seeded with `task_tools=('definitely-not-running.exe','tool')` so real foreground reads off-task):
  prompting→Start click→started → §6.4 on-task check-in fired at cadence → task-list picker rendered
  ("What's actually due", red-outline <24h row, Take a break) → row click `PickTask` → classification
  screen (`[Tool][Not a tool][Ignore]`+Done) → Tool click persisted `(task,'claude.exe','tool')` to
  `task_tools`, auto-Done → started. A separate user-driven real-mouse pass persisted
  `('brave.exe','not_tool')` to `app_classes` (global NotTool path). All verified via sessions.db edges
  + PrintWindow captures. Tray Pause submenu still unexercised (needs real tray click).
- Method notes: FindWindowW needs `IntPtr::Zero` title (empty-string binds to exact-match and misses);
  classes `NudgeTaskList`(520w)/`NudgeClassify`(560w); row y = 34+15, classify Tool btn x≈352.
- **Review pass (low)**: 4 findings, none blocking — (1) re-Pause while Paused loses `was_started`
  (`was_live` doesn't match `Paused`); (2)+(3) CheckIn/Choosing don't carry `task_id`, so any check-in
  resolution/classification rebinds to `window_task_id`, dropping a Tier-C pick; (4) `logged_minutes`
  accrual truncates `sample_secs/60` → 0 for sub-minute cadences. Candidates for a follow-up step.
- Step 3 ticked in NEXTSTEPS. Not committed.

## Session 55 (2026-07-17) — Step 3: renderers consume `meta.style_bands`
- `persist::Db` (nudge-svc) gained a `meta` table (byte-identical to nudge-app's `db.rs` one — both
  crates open the same `sessions.db`) + read-only `get_meta(key)`.
- `tasklist.rs`: `outline(style, bands)` takes the §6.9 palette as a parameter instead of always reading
  the built-in `OUTLINE_BANDS` const — empty `bands` (unset setting) falls back to it. New
  `parse_bands`/`parse_hex_color` turn `#RRGGBB` strings (the format `set_style_bands` writes) into
  `COLORREF`s, dropping any entry that fails to parse rather than discarding the whole palette. The
  paint-state static (`ROWS`) widened to carry `Vec<COLORREF>` alongside rows/now so `WM_PAINT` can read
  it; `TaskList::create` takes `bands: &[COLORREF]`.
- `main.rs`: `Effect::ShowTaskList` reads `meta.style_bands` fresh every time the list is about to show,
  parses it, and passes the result to `TaskList::create`. Chose "read at show time" over threading it
  through rules reload because the Style tab has no reload-signal of its own (unlike rules.toml edits) —
  re-reading per-show is one query and stays correct without adding a new signal path.
  `overlay.rs` (the anchor strip) never referenced band colors, so it needed no change.
- 153 workspace tests green (up from 148: +5 tasklist — empty/custom/clamp outline cases,
  `parse_bands` good/bad-entry handling). `cargo build -p nudge-svc` and nudge-app `cargo build` both
  clean (only pre-existing warnings). Not live-driven: verifying a visible band-color change needs a
  live check-in list up at the same time as a Style-tab edit in the running app, which wasn't set up
  this session — static/unit coverage only. Not committed.

## Session 56 (2026-07-17) — Step 4: deadline-only tasks + undated-once suggestions
- `schedule::Win::from_task` previously bailed (`t.minutes?`) whenever a task had no `minutes`
  time-of-day, so a deadline-only task (Weekly with no cue, or a Once task whose deadline only encodes
  a date) never produced a window at all. Added `DEFAULT_TASK_MINUTES` (09:00) as the fallback: `start =
  t.minutes.unwrap_or(DEFAULT_TASK_MINUTES)`, applied after the `Once`-without-deadline `?` short-circuit
  (that case still correctly returns `None` — no date to anchor a one-shot firing to, stays a
  planner-only row, unchanged from before).
- `accept_suggested_trigger` (nudge-app `db.rs`) already degrades gracefully for an undated suggestion
  (`deadline.map(local_minutes_of_day)` → `minutes: None`), so no change was needed there — the fix in
  `schedule.rs` is what makes the resulting deadline-only task actually fire instead of silently never
  scheduling. Confirmed by reading through, not by touching the file.
  Left alone by design: `Recur::Once` with **no deadline** (e.g. an undated Gmail suggestion accepted
  as-is) still stays a planner-only row with no window — there is no calendar date to anchor a one-shot
  firing to, and firing it daily would break "once" semantics. Considered contentious enough to flag
  rather than guess; scoped out of this step.
- Tests: replaced `deadline_only_and_undated_once_skipped` (now-stale expectation) with
  `undated_once_skipped` (unchanged behavior, isolated) + two new tests —
  `deadline_only_weekly_task_uses_default_minutes` and `deadline_only_once_task_uses_default_minutes` —
  covering the new fallback for both `Recur` variants.
- `cargo test --workspace`: 153 passed, 0 failed (nudge-core 102 + core_integration 1 + nudge-draft 9 +
  nudge-svc 41; nudge-ctl and doc-tests contribute 0 either way). Compiles the whole tree, so this
  doubles as the build check — no separate `cargo build --workspace` run. Not committed.

## Session 57 (2026-07-17) — Step 5: custom pause duration input
- Investigated a native text-input surface first: `nudge-svc` has none anywhere (no `EDIT`-class
  child window, no `MessageBox`/`DialogBox` call in the whole crate), and `overlay.rs`'s layered
  strip (`Anchor`, used for the prompt/check-in/task-list windows) is a click-only display surface —
  building a real free-text Win32 dialog was judged out of scope for a ~1h Sonnet step.
- Went with the config-driven alternative flagged in `PLAN-step3.md` D.2 and `tray.rs`'s own doc
  comment: a new `[escalation] custom_pause_secs` rules.toml field (`rules.rs`, default 3600s,
  validated alongside the other durations — negative rejected same as `pause_secs` etc.), and a
  "Custom (rules)" item appended to the existing tray Pause submenu (`tray.rs`) parallel to the
  pre-existing "Default (rules)" item. New plumbing, mirroring the Default item's path exactly:
  `TrayCmd::PauseCustom` (tray.rs) → `LoopSignal::PauseCustom(UnixTime)` (timers.rs) →
  `Event::PauseFor(t, rules.escalation.custom_pause_secs)` (main.rs). The user sets a one-off pause
  length by editing rules.toml and clicking Tray > Reload rules — no restart, reuses the live-reload
  path that already exists for every other rules.toml edit.
- `rules.example.toml` documents the new field next to `pause_secs`.
- New test: `rules.rs::custom_pause_secs_defaults_and_overrides` (default 3600, override to 900,
  rejects negative) — same pattern as the other escalation-field tests.
- `cargo test --workspace`: 154 passed, 0 failed (nudge-core 103, +1 from this session; core_integration
  1 + nudge-draft 9 + nudge-svc 41 unchanged). `cargo build --workspace` clean (2 pre-existing warnings,
  unrelated: `hotkey::Trigger` never used, `persist::app_class` never used). Not committed. Not
  live-driven — config parsing + tray-menu-id plumbing only, same risk class as the already
  live-verified "Default (rules)" item (session 54), so static verify was judged sufficient.

## Session 63 (2026-07-19) — Step 8: Gmail scan 403 → actionable reconnect message
- Confirmed the scope side is already correct: `google/mod.rs` `SCOPES` includes
  `https://www.googleapis.com/auth/gmail.readonly` (added in 10e), and the reconnect path exists —
  `google_disconnect` drops the cached token, `google_connect` re-runs full consent (`prompt=consent`),
  so the runtime fix for a pre-10e calendar-only token is a user **Reset connection → Reconnect**. The
  `GoogleConnect.svelte` panel already renders both buttons plus a "Gmail scan returning 403?" hint.
- The missing piece was the error surface: a 403 from `users.messages.list` propagated as the raw
  `ureq` status line ("messages.list request failed: http status: 403"). Added `request_err(context, e)`
  in `google/gmail.rs` — on `ureq::Error::Status(403, _)` it returns an actionable string
  ("Reconnect Google to grant mail access … click Reset connection, then Reconnect and approve the Gmail
  permission"); every other status/transport error keeps the plain `{context}: {e}` shape. Wired into
  both `list_message_ids` and `get_message`. The scan command bubbles the string straight to the toast,
  so the user sees the fix instead of an HTTP code.
- Tests: `request_err_403_prompts_reconnect` (403 → reconnect text, no raw context leak) and
  `request_err_other_status_keeps_context` (500 → keeps `{context}:` prefix). `cargo test --lib
  google::gmail`: 7 passed, 0 failed. Backend-only, no svc change. Not committed. The remaining runtime
  step (actually reconnecting Google) is a user action the UI now clearly directs.

## Session 68 (2026-07-19) — Step 11e / PLAN-bulk-upload P5: `BulkUpload.svelte` + Tasks-page entry
- New `crates/nudge-app/src/lib/BulkUpload.svelte` implementing §6 UI. One unified `rows` state
  array; an `ignored` flag + `reason` decide whether a row renders in the report block ("Ignored
  duplicates") or the valid table ("New tasks"). Both tables are inline-editable via the same cell
  inputs, including the leading **Group**/**Project** columns.
- Bidirectional, rule-driven row movement (§5) with the client never deciding dedup: every cell
  `onchange` calls `revalidateAll()`, which re-serializes ALL current rows to a canonical 10-col CSV
  and re-invokes `validate_bulk_upload`; the fresh `new_rows`/`ignored_rows` split rebuilds both
  tables. Fixing a dup's deadline promotes it to the valid table; reintroducing a collision demotes a
  valid row to the report block. Include-toggles survive a re-validate via a content signature set.
- File loading: `.csv`/`.xlsx`/`.xlsm` via `FileReader.readAsArrayBuffer` → `Array.from(Uint8Array)`
  byte array + extension; CSV paste UTF-8 encodes with `ext = "csv"`. Confirm sends
  `[{form, project_group, project}]` to `confirm_bulk_upload` (server re-validates + re-dedups + one tx).
- Wiring: `validateBulkUpload`/`confirmBulkUpload` added to `api.js`; `bulkUploadConfirm` store helper
  (confirm → `refresh`). The standalone **Import** rail tab and its `CsvImport` render were removed from
  `App.svelte`; Bulk Upload is now a "⤓ Bulk Upload" button in the Tasks-page header toggling an
  in-page sub-view (with a "← Tasks" back button). `CsvImport.svelte` left on disk, now unreferenced.
- `npm run build` green (132 modules, no errors). Frontend-only; no svc/backend change. Not committed.
  Next: 11f verify — unit matrices + live-drive a mixed `.xlsx` end-to-end.

## Session 69 (2026-07-19) — Step 11f / PLAN-bulk-upload P6: verify — **Step 11 (Bulk Upload) complete**
- Found the on-disk release exe stale (Jul 18, pre-`BulkUpload.svelte`). Rebuilt: `vite build` +
  `cargo build --release` from `crates/nudge-app/src-tauri` (exe 03:10, 13.2 MB).
- **Headless end-to-end drive of the ingest pipeline** (below the Tauri IPC boundary — the
  `validate_bulk_upload`/`confirm_bulk_upload` commands at `lib.rs:228`/`:269` are thin wrappers over
  `read_spreadsheet → parse_import → dedup_rows` then `form_to_task → try_reserve → insert_bulk`).
  Added permanent test `import_e2e_tests::mixed_xlsx_bulk_upload_end_to_end`: builds a mixed `.xlsx`
  (two groups + blank-group row + title dup (NOCASE) + deadline dup), runs both command bodies against
  a temp `Store`, asserts the **report partition (4 new / 2 ignored, title+deadline reasons checked
  individually)** and, by reopening the DB file, the **end-state**: 2 groups + 2 projects auto-created,
  same group+project rows share one `project_id`, blank-group row `project_id` NULL, 4 tasks in one tx.
- Also drove the **actual openpyxl-generated fixture** (`scratchpad/mixed_bulk.xlsx`) through
  `read_spreadsheet` (real calamine path) → 4 new / 2 ignored, reasons `title "send revised budget"
  already exists` + `deadline 1784865600 already exists` — proves calamine reads a third-party `.xlsx`
  (shared strings/styles), not just `rust_xlsxwriter` output. (Temporary env-gated test, since removed.)
- **81 app-lib tests green** (80 prior + the new xlsx e2e).
- **GUI layer NOT driven** — computer-use approval is unavailable in scheduled runs (`request_access`
  returned "can't be approved during a scheduled run"). Launching the app from Claude's MSIX-redirected
  shell also virtualizes the DB path, so the live `%LOCALAPPDATA%\nudge-bot\sessions.db` was untouched
  (still pre-§7.1 `trigger_source`, no `project_id`). The Svelte↔Tauri wiring (arraybuffer→invoke,
  two-table render, §5 bidirectional row movement) is deferred to a human-present session → NEXTSTEPS
  step 1. Nothing committed.

## Session 70 (2026-07-19) — Step 2 / UI-PLAN §7.2 phase 1: Task↔Event auto-tie on create (backend)
- Started §7.2 (bidirectional Task↔Event binding). First coherent, offline-verifiable phase:
  **auto-tie on create** — creating a Task now creates a bound Calendar Event and persists the binding.
- `db.rs`: `Store::set_task_gcal_event_id(id, Option<&str>) -> usize` (UPDATE tasks SET gcal_event_id;
  0 rows on unknown id). Test `set_task_gcal_event_id_binds_and_clears` (bind → clear → unknown no-op).
- `lib.rs`: pure `default_event_bounds(deadline, duration_secs) -> (start, end)` (§7.2 default Deadline →
  Deadline + 1 h; test `default_event_bounds_is_deadline_plus_duration`). `try_autotie(store, id, title,
  deadline)` — best-effort, non-fatal: returns `None` (task stays unbound) when the task has no Deadline,
  no primary calendar is chosen, or Google token/`create_event` fails (GOOGLE-PLAN #4 offline-degrades).
  On success: `create_event` on the primary calendar (start=Deadline, end=Deadline+duration; duration
  from meta `default_event_secs`, else 3600), caches the event locally (`upsert_event`), binds via
  `set_task_gcal_event_id`. Wired into `insert_and_reload` (covers both `add_task` and `add_quickadd`);
  DTO now returns the bound `gcal_event_id`. **Suggested-exclusion (§7.2) already satisfied** —
  `connectors.rs` excludes `known_gcal_event_ids()`, so a now-bound task drops out of Suggested for free.
- **83 app-lib tests green** (81 prior + 2). No commit.
- **Deferred to later §7.2 phases:** the live network drive of auto-tie (needs OAuth + a chosen primary
  calendar — not available in a scheduled run); **edit propagation** (blocked on there being no
  `update_task` command yet — tasks are create/delete-only in the app today); event-click Add/Edit/Delete
  menu (§7.2, calendar UI); Svelte surfacing of the binding.
