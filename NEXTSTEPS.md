# NEXTSTEPS.md
<!-- Boundary: forward-looking task queue only. Completed-step detail lives in HISTORY.md.
     Repo is live at github.com/harris11ax/nudge-bot-app — see "GitHub Workflow" below. -->

Last completed: session 66 — Step 11c (Bulk Upload P3: `.xlsx` reader — `read_spreadsheet(bytes,ext)` seam in `csv_import.rs`; calamine first-worksheet→CSV→`parse_import`; 78 app-lib tests green).
Next open: **step 11d** (Bulk Upload P4 — tauri commands, Sonnet, [PLAN-bulk-upload.md](PLAN-bulk-upload.md) §7 P4),
**step 9** (UI-PLAN §7 rework — planned, [UI-PLAN.md](UI-PLAN.md) §7).
Step 6 (nudge-draft LLM pass) is PARKED. Full history: [HISTORY.md](HISTORY.md).

## Open steps
- [ ] **9 — UI-PLAN §7 rework: Tasks nomenclature, Task↔Event binding, Settings/Calendar, Tools launch** | **Opus** | ~2–3d | **PLANNED → [UI-PLAN.md](UI-PLAN.md) §7**
  Requested 2026-07-18. Four areas: (a) **§7.1** rename Trigger→Task + start time→**Deadline** app-wide, reorder Tasks tab (Quick Add top / Suggested bottom), `trigger_source`→`task_source`; (b) **§7.2** bidirectional Task↔Calendar Event binding (auto-tie on create, default event = Deadline→Deadline+1h, edit propagation to GCal, Task-bound events excluded from Suggested, calendar event-click menu Add Task/Edit Event/Delete Event); (c) **§7.3** dedicated Settings → Calendar tab; (d) **§7.4** relabel/hide AW "Unknown (system/lock screen)" bucket, per-website resolution inside browsers, Launch-tool-from-task (web URL + Windows exe). Phase per session. **Resolved (session 58):** "Deadline" is a **field-semantics change** — `deadline` (hard unix deadline, already in the model + the sole key of `display_list`) becomes the primary user-facing task time; the fire `minutes` derives from it (same pattern as step 4 + CSV import). **§7.1 backend DONE (session 58):** `trigger_source`→`task_source` renamed across DB column (both crates, byte-identical CREATE), DTO/JSON API, and `Task` field; back-compat `RENAME COLUMN` migration in both crates + a legacy-DB migration test; internal `TriggerSource` type kept. Workspace 41 + app 58 tests green. **§7.1 DONE (session 60):** Svelte relabel — tab "Triggers"→"Tasks" (App.svelte label; internal id kept), Planner "New task", Tasks page header "Tasks" / "Full task" / "Create task"; Deadline-first binding — dropped standalone Time-of-day field, `minutes` now derives from Deadline's time-of-day (`minutesFromDeadline`); tab reorder — Quick Add + Full task on top, Suggested moved to bottom. `npm run build` green. **§7.1 fully complete; next phase: §7.2** (Task↔Event binding).

- [ ] **11 — Bulk Upload: task hierarchy + spreadsheet ingest** | **PLANNED → [PLAN-bulk-upload.md](PLAN-bulk-upload.md)**
  Requested 2026-07-19. Extends step 7 (CSV import). Adds a Project Groups → Projects → Tasks hierarchy
  (`tasks.project_id` nullable FK, one task ↔ one project), `.xlsx` ingest, a rename Import→**Bulk Upload**
  reached from the Tasks page, and title/deadline **dedup with an ignored-rows report block** ahead of an
  editable confirm table. Spreadsheet gains two leading columns `project_group,project` that resolve
  (exact NOCASE match-or-create) per row. Dedup rule: a row is new iff **both** title AND deadline are
  unused; if **either** matches an existing task it's reported ignored. App-process only — no svc change,
  no polling, no in-app LLM/network. Phases (one per session, each references the plan):
  - [x] **11a — schema + hierarchy resolver** | **Sonnet** | ~0.5d | PLAN §7 P1 — DONE (session 64): `project_groups`/`projects` tables in `db.rs` CREATE block, tolerant `ALTER TABLE tasks ADD COLUMN project_id`, `resolve_group_project(group,project)->Option<i64>` (exact NOCASE match-or-create, trimmed; blank-group & blank-project→None; project-without-group→Err). Tests `resolve_group_project_match_or_create` + `project_id_migration_is_tolerant`; 7 db:: tests green.
  - [x] **11b — parser + dedup** | **Sonnet** | ~0.5d | PLAN §7 P2 — DONE (session 65): `parse_import` extended to the 10-col header (`project_group`+`project`, `group` alias, project-without-group row error); pure `dedup_rows(rows,&mut ExistingKeys)->DedupOutcome{new_rows,ignored}` per §5 (new iff title NOCASE-trimmed AND deadline both unused; else ignored w/ matched reason; empty deadline collides only with empty; intra-sheet first-wins; invalid rows bypass, don't reserve keys). Tests: group/project capture+alias+error + 7 dedup cases; 76 app-lib tests green.
  - [x] **11c — `.xlsx` reader** | **Sonnet** | ~0.5d | PLAN §7 P3 — DONE (session 66): `read_spreadsheet(bytes,ext)` seam in `csv_import.rs` — `.csv` UTF-8 passthrough, `.xlsx`/`.xlsm` via `calamine` (first worksheet → quoted CSV via `csv::Writer` → `parse_import`), whole-float→int cell normalization (estimate `90.0`→`90`), unsupported-ext error. `calamine` dep + `rust_xlsxwriter` dev-dep; tests `read_spreadsheet_csv_passthrough_and_bad_ext` + `read_spreadsheet_xlsx_feeds_parse_import` (in-memory fixture round-trip); 78 app-lib tests green. CSV path untouched.
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
- [x] **8** — Gmail scan 403 → actionable reconnect message (`request_err` in `google/gmail.rs`; scope + reconnect path already correct) (session 63).
- [x] **7** — CSV bulk task import: P1 parser (58) · P2 `insert_batch`+cmds (59) · P3 `CsvImport.svelte` filter screen (61) · P4 batch/e2e tests, 61 app-lib tests green (session 62).
- [x] **11a** — Bulk Upload P1: `project_groups`/`projects` tables + tolerant `project_id` ALTER + `resolve_group_project` match-or-create in `db.rs` (session 64).
- [x] **11b** — Bulk Upload P2: 10-col parser (`project_group`+`project`) + pure `dedup_rows` per PLAN §5; 76 app-lib tests green (session 65).
- [x] **11c** — Bulk Upload P3: `.xlsx` reader — `read_spreadsheet(bytes,ext)` seam (calamine first-worksheet→CSV→`parse_import`); 78 app-lib tests green (session 66).

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
