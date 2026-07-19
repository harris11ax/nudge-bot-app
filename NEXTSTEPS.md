# NEXTSTEPS.md
<!-- Boundary: forward-looking task queue only. Completed-step detail lives in HISTORY.md.
     Repo is live at github.com/harris11ax/nudge-bot-app — see "GitHub Workflow" below. -->

Last completed: session 62 — Step 7 P4 (CSV import tests: `insert_batch` atomicity/ordering, empty no-op, end-to-end mixed valid/broken pipeline). Step 7 **fully complete**.
Next open: **step 9** (UI-PLAN §7 rework — planned, [UI-PLAN.md](UI-PLAN.md) §7), **step 8** (fix Gmail scan 403),
and **step 11** (Bulk Upload: hierarchy + spreadsheet ingest — planned, [PLAN-bulk-upload.md](PLAN-bulk-upload.md)).
Step 6 (nudge-draft LLM pass) is PARKED. Full history: [HISTORY.md](HISTORY.md).

## Open steps
- [ ] **9 — UI-PLAN §7 rework: Tasks nomenclature, Task↔Event binding, Settings/Calendar, Tools launch** | **Opus** | ~2–3d | **PLANNED → [UI-PLAN.md](UI-PLAN.md) §7**
  Requested 2026-07-18. Four areas: (a) **§7.1** rename Trigger→Task + start time→**Deadline** app-wide, reorder Tasks tab (Quick Add top / Suggested bottom), `trigger_source`→`task_source`; (b) **§7.2** bidirectional Task↔Calendar Event binding (auto-tie on create, default event = Deadline→Deadline+1h, edit propagation to GCal, Task-bound events excluded from Suggested, calendar event-click menu Add Task/Edit Event/Delete Event); (c) **§7.3** dedicated Settings → Calendar tab; (d) **§7.4** relabel/hide AW "Unknown (system/lock screen)" bucket, per-website resolution inside browsers, Launch-tool-from-task (web URL + Windows exe). Phase per session. **Resolved (session 58):** "Deadline" is a **field-semantics change** — `deadline` (hard unix deadline, already in the model + the sole key of `display_list`) becomes the primary user-facing task time; the fire `minutes` derives from it (same pattern as step 4 + CSV import). **§7.1 backend DONE (session 58):** `trigger_source`→`task_source` renamed across DB column (both crates, byte-identical CREATE), DTO/JSON API, and `Task` field; back-compat `RENAME COLUMN` migration in both crates + a legacy-DB migration test; internal `TriggerSource` type kept. Workspace 41 + app 58 tests green. **§7.1 DONE (session 60):** Svelte relabel — tab "Triggers"→"Tasks" (App.svelte label; internal id kept), Planner "New task", Tasks page header "Tasks" / "Full task" / "Create task"; Deadline-first binding — dropped standalone Time-of-day field, `minutes` now derives from Deadline's time-of-day (`minutesFromDeadline`); tab reorder — Quick Add + Full task on top, Suggested moved to bottom. `npm run build` green. **§7.1 fully complete; next phase: §7.2** (Task↔Event binding).

- [ ] **8 — Fix "Scan Gmail + Calendar" 403** | **Sonnet** | ~0.5–2h
  Reported 2026-07-17: `messages.list ... status code 403` on the inbox scan. Likely a **stale OAuth scope** —
  `gmail.readonly` was added last (step 10e, google/mod.rs SCOPES); a Google connection made before 10e holds
  only calendar scopes, so the cached token can't call `users.messages.list` (403 insufficient permission).
  Fix: (1) confirm `TokenCache.scope` (google/mod.rs) lacks `gmail.readonly`; (2) primary fix — **reconnect
  Google** (`google_connect` re-consents with `prompt=consent`) and re-scan; (3) if still 403, verify the
  **Gmail API is enabled** in the GCP project (console setting, not code). Add a UI hint: on a Gmail 403,
  surface "Reconnect Google to grant mail access" instead of the raw error. Calendar half is unaffected.

- [ ] **11 — Bulk Upload: task hierarchy + spreadsheet ingest** | **PLANNED → [PLAN-bulk-upload.md](PLAN-bulk-upload.md)**
  Requested 2026-07-19. Extends step 7 (CSV import). Adds a Project Groups → Projects → Tasks hierarchy
  (`tasks.project_id` nullable FK, one task ↔ one project), `.xlsx` ingest, a rename Import→**Bulk Upload**
  reached from the Tasks page, and title/deadline **dedup with an ignored-rows report block** ahead of an
  editable confirm table. Spreadsheet gains two leading columns `project_group,project` that resolve
  (exact NOCASE match-or-create) per row. Dedup rule: a row is new iff **both** title AND deadline are
  unused; if **either** matches an existing task it's reported ignored. App-process only — no svc change,
  no polling, no in-app LLM/network. Phases (one per session, each references the plan):
  - [ ] **11a — schema + hierarchy resolver** | **Sonnet** | ~0.5d | PLAN §7 P1 — new `project_groups`/`projects` tables + tolerant `project_id` ALTER; `resolve_group_project` match-or-create; temp-DB unit tests.
  - [ ] **11b — parser + dedup** | **Sonnet** | ~0.5d | PLAN §7 P2 — extend `parse_import` to the 10-col header; pure `dedup_rows` over an injected existing-set; unit matrix (title/deadline/both-empty/intra-sheet/blank-group).
  - [ ] **11c — `.xlsx` reader** | **Sonnet** | ~0.5d | PLAN §7 P3 — `calamine` behind `read_spreadsheet(bytes,ext)` feeding `parse_import`; fixture `.xlsx` test; CSV path untouched.
  - [ ] **11d — tauri commands** | **Sonnet** | ~0.5d | PLAN §7 P4 — `validate_bulk_upload` + `confirm_bulk_upload` (one transaction: resolve hierarchy + `insert_batch` + single reload; server-side re-validate/re-dedup); `generate_handler!`.
  - [ ] **11e — `BulkUpload.svelte` + Tasks-page entry** | **Opus** | ~1–1.5d | PLAN §7 P5 — rename Import→Bulk Upload under Tasks page; **both** tables editable (valid + ignored report block) with bidirectional rule-driven row movement (fix a dup's deadline → jumps to valid; reintroduce a collision → drops to report); Group/Project columns; Confirm; api.js wrappers + store batch helper. *(Opus: cross-cutting UI/state + in-table hierarchy editing + two-way row migration.)*
  - [ ] **11f — verify** | **Sonnet** | ~0.5d | PLAN §7 P6 — unit matrices green; live-drive a mixed `.xlsx` (two groups + title dup + deadline dup + blank-group row) end-to-end.

- [ ] **6 — nudge-draft LLM title/time pass** | **Opus** | ~1d | **PARKED (2026-07-17)**
  LLM pass to draft task titles/times from Gmail/GCal candidates. Needs the `nudge-draft` binary crate split
  into lib+bin first. Parked by user: tasks are added manually / via spreadsheet, so the deterministic
  connector heuristics (`connectors.rs`) are the shipped floor. Design notes for whoever revives it: HISTORY.md.

## Done (one-line; detail in HISTORY.md)
- [x] **10e** — Gmail/GCal connectors + `Recur::Once` date-anchored firing (session 41).
- [x] **1** — UI addendum: task tools, check-in flow, dynamic deadline windows, style settings (session 43).
- [x] **2 / 2b** — Drive P4 UI end-to-end; button-paint + off-task check-in confirmed non-bugs (sessions 44–46).
- [x] **3** — Tier B: on-task check-in + tool classification screen + Tools selector; planned in [PLAN-step3.md](PLAN-step3.md) (sessions 47–52).
- [x] **1** — Fix session-52 review findings: task carry + pause + accrual (session 53).
- [x] **2** — Verify tray Pause submenu with a real tray click (session 54).
- [x] **3** — Renderers consume `meta.style_bands` (session 55).
- [x] **4** — Deadline-only tasks + undated-once suggestions (session 56).
- [x] **5** — Custom pause duration via `[escalation] custom_pause_secs` (session 57).
- [x] **7** — CSV bulk task import: P1 parser (58) · P2 `insert_batch`+cmds (59) · P3 `CsvImport.svelte` filter screen (61) · P4 batch/e2e tests, 61 app-lib tests green (session 62).

## Session model: Sonnet (default) | Opus gate on planning/complex design | Haiku for trivial tasks
- Read NEXTSTEPS.md at start. Scan for incomplete steps:
  - **Priority 1**: unskippable Sonnet step → proceed without prompting.
  - **Priority 2**: any Sonnet step → proceed without prompting.
  - **Priority 3**: no Sonnet steps left → apply Opus/Haiku gates (state the gate, wait for the user's model reply).
- Complete ONE step per session. Update the checkbox on done, log the session to HISTORY.md via the nextsteps-classifier skill.
- If all done: confirm completion. If NEXTSTEPS.md absent/empty: scan for planned features, add the next logical step.

## GitHub Workflow
- **Remote**: `https://github.com/harris11ax/nudge-bot-app.git`, `main` tracking `origin/main`.
- **Uncommitted**: only `ade2023` ("Initial commit") is pushed; all work since (sessions 2–57) is local-only.
  Don't claim anything is "on GitHub" unless `git log`/`git status` confirms it.
- **Branches**: `main` is production-ready; feature work on `feature/…` / `fix/…`, one NEXTSTEPS step per branch/PR.
- **Commit messages**: reference the NEXTSTEPS step (e.g. "7: CSV import parser").
- **PRs**: via `gh pr create` once `gh` is authenticated here (not currently on PATH).
- **Commits/pushes are explicit-permission actions** — write to disk and report readiness; don't `add`/`commit`/`push` unless the user asks in the same turn.

## Open Decisions
- Max snoozes per prompt? (currently unlimited, each logged.)
- Check-in is global-only; per-window override (`[[nudge]].checkin_after_secs`) if wanted later.
- Productive-app list global-only for now, per-task later? Task-type taxonomy for filters — user to supply initial list.
