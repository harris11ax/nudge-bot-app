# NEXTSTEPS.md
<!-- Boundary: forward-looking task queue only. Completed-step detail lives in HISTORY.md.
     Repo is live at github.com/harris11ax/nudge-bot-app — see "GitHub Workflow" below. -->

Last completed: session 73 — Step 2 / UI-PLAN §7.2 **Delete Event (backend command + Svelte surfacing)**. New `delete_event(event_id)` tauri command (lib.rs): pushes `google::calendar::delete_event` (new `DELETE /events/{id}` API fn — `410 Gone` treated as success) **before** touching local state, then `Store::delete_event` drops the cached row and new `Store::clear_tasks_bound_to_event` unbinds every task pointing at it (signals reload if any). `api.js deleteEvent` wrapper; `Calendar.svelte` edit dialog gains a **Delete** button (`deleteDialogEvent` → refreshes grid + task store so a bound task's Calendar-event flips to "—"). Registered in `generate_handler!`. 85 app-lib tests green (+`clear_tasks_bound_to_event_unbinds_matching_only`); `npm run build` green (132 modules).
Next open: **step 1** (GUI drive of BulkUpload.svelte — human-present, [PLAN-bulk-upload.md](PLAN-bulk-upload.md) §6),
**step 2** remaining phase: **§7.2 event-click Add Task** (create a task from an existing event — Edit Event + Delete Event now done) + live network drive ([UI-PLAN.md](UI-PLAN.md) §7.2, needs OAuth + chosen primary calendar — human-present).
Step 6 (nudge-draft LLM pass) is PARKED. Full history: [HISTORY.md](HISTORY.md).

## Open steps
- [ ] **1 — GUI drive of `BulkUpload.svelte` (human-present verification)** | **Sonnet** | ~30m | **NEEDS INTERACTIVE SESSION → [PLAN-bulk-upload.md](PLAN-bulk-upload.md) §6**
  Deferred from 11f: the Rust ingest+persist pipeline is fully verified (81 tests), but the webview layer on top
  was never driven because computer-use approval can't be granted in a scheduled run. In a human-present session,
  launch the fresh release build (`scripts/launch-silent.vbs`; note: launching from Claude's MSIX-redirected shell
  virtualizes the DB path — launch outside the container), open Tasks → **⤓ Bulk Upload**, upload the mixed fixture
  at `scratchpad/mixed_bulk.xlsx` (or rebuild: two groups + blank-group row + title dup + deadline dup), and confirm:
  (a) report block shows exactly the 2 dupes with reasons, (b) valid table shows 4 rows w/ resolved Group/Project,
  (c) editing an ignored row's deadline moves it **up** to the valid table (and a valid-row edit that reintroduces a
  collision moves it **down**) — the §5 bidirectional movement, (d) Confirm writes 4 tasks + one reload.

- [ ] **2 — UI-PLAN §7 rework: Tasks nomenclature, Task↔Event binding, Settings/Calendar, Tools launch** | **Opus** | ~2–3d | **PLANNED → [UI-PLAN.md](UI-PLAN.md) §7**
  Requested 2026-07-18. Four areas: (a) **§7.1** rename Trigger→Task + start time→**Deadline** app-wide, reorder Tasks tab (Quick Add top / Suggested bottom), `trigger_source`→`task_source`; (b) **§7.2** bidirectional Task↔Calendar Event binding (auto-tie on create, default event = Deadline→Deadline+1h, edit propagation to GCal, Task-bound events excluded from Suggested, calendar event-click menu Add Task/Edit Event/Delete Event); (c) **§7.3** dedicated Settings → Calendar tab; (d) **§7.4** relabel/hide AW "Unknown (system/lock screen)" bucket, per-website resolution inside browsers, Launch-tool-from-task (web URL + Windows exe). Phase per session. **Resolved (session 58):** "Deadline" is a **field-semantics change** — `deadline` (hard unix deadline, already in the model + the sole key of `display_list`) becomes the primary user-facing task time; the fire `minutes` derives from it (same pattern as step 4 + CSV import). **§7.1 backend DONE (session 58):** `trigger_source`→`task_source` renamed across DB column (both crates, byte-identical CREATE), DTO/JSON API, and `Task` field; back-compat `RENAME COLUMN` migration in both crates + a legacy-DB migration test; internal `TriggerSource` type kept. Workspace 41 + app 58 tests green. **§7.1 DONE (session 60):** Svelte relabel — tab "Triggers"→"Tasks" (App.svelte label; internal id kept), Planner "New task", Tasks page header "Tasks" / "Full task" / "Create task"; Deadline-first binding — dropped standalone Time-of-day field, `minutes` now derives from Deadline's time-of-day (`minutesFromDeadline`); tab reorder — Quick Add + Full task on top, Suggested moved to bottom. `npm run build` green. **§7.1 fully complete.** **§7.2 phase 1 DONE (session 70):** Task↔Event **auto-tie on create** — `Store::set_task_gcal_event_id` (db.rs); pure `default_event_bounds(deadline,dur)` = Deadline→Deadline+1h (dur from meta `default_event_secs`, else 3600); best-effort non-fatal `try_autotie` (unbound when no Deadline / no primary calendar chosen / Google token or `create_event` fails — GOOGLE-PLAN #4) creating the event on the primary calendar, caching it (`upsert_event`) and binding it; wired into `insert_and_reload` (covers `add_task`+`add_quickadd`), DTO returns `gcal_event_id`. Suggested-exclusion already satisfied (`connectors.rs` excludes `known_gcal_event_ids`). 83 app-lib tests green. **§7.2 phase 2 DONE (session 71):** edit-propagation backend — `Store::get`/`Store::update` (editable-fields-only in-place update, binding/origin/hierarchy/accrual preserved) + `update_task(id,NewTaskForm)` command (best-effort `try_propagate_event` pushes title/Deadline onto the bound event, non-fatal). 84 app-lib tests green. **§7.2 Svelte surfacing DONE (session 72):** `api.js` `updateTask` + `store.svelte.js` `saveTask`; `TaskPage.svelte` ✎ Edit inline form → `update_task` (prefilled, Deadline-derived `minutes`, adopts returned DTO's refreshed binding), detail `dl` shows Calendar-event bound/—; `npm run build` green (132 modules). **§7.2 Delete Event DONE (session 73):** `delete_event(event_id)` command — `google::calendar::delete_event` (`DELETE /events/{id}`, `410`→ok) pushed before local mutation, then `Store::delete_event` + new `Store::clear_tasks_bound_to_event` (unbind matching tasks, reload if any); `api.js deleteEvent` + `Calendar.svelte` edit-dialog **Delete** button; 85 app-lib tests green. **Remaining §7.2:** event-click **Add Task** (create a task from an existing event — Edit Event via edit dialog + Delete Event now done); live network drive (needs OAuth + chosen primary calendar — human-present).

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
  - [x] **11d — tauri commands** | **Sonnet** | ~0.5d | PLAN §7 P4 — DONE (session 67): `validate_bulk_upload(bytes,ext)->{new_rows,ignored_rows}` (read_spreadsheet→parse_import→dedup_rows vs live `list()` snapshot) + `confirm_bulk_upload(rows)->usize` (per-row `form_to_task` re-validate + `ExistingKeys::try_reserve` re-dedup, then one transaction resolve+insert+single `signal_reload`); both in `generate_handler!`. `resolve_group_project` refactored to a connection-level free fn shared with new `Store::insert_bulk` (resolve hierarchy + insert w/ `project_id` in one tx, aborts on project-without-group). Tests `insert_bulk_resolves_hierarchy_and_project_id` + `insert_bulk_aborts_on_unmappable_row`; 80 app-lib tests green.
  - [x] **11e — `BulkUpload.svelte` + Tasks-page entry** | **Opus** | ~1–1.5d | PLAN §7 P5 — DONE (session 68): `BulkUpload.svelte` — report block (ignored dupes + reason) and valid "New tasks" table both inline-editable via one unified `rows` model; **any cell edit re-serializes ALL rows to a 10-col CSV and re-calls `validate_bulk_upload`**, so rows migrate bidirectionally between the two tables by the §5 rule alone (server owns dedup; client never decides). Group/Project columns editable; `.csv`/`.xlsx`/`.xlsm` file pick (`readAsArrayBuffer`→byte array) + CSV paste; include-toggle preserved across re-validate via content signature; Confirm→`confirm_bulk_upload`. `validateBulkUpload`/`confirmBulkUpload` api.js wrappers + `bulkUploadConfirm` store helper. Standalone Import rail tab + `CsvImport` render removed from `App.svelte`; entry now a "⤓ Bulk Upload" button in the Tasks-page header toggling a sub-view. `npm run build` green (132 modules).
  - [x] **11f — verify** | **Sonnet** | ~0.5d | PLAN §7 P6 — DONE (session 69): fresh release build; permanent self-contained e2e test `mixed_xlsx_bulk_upload_end_to_end` mirrors `validate_bulk_upload`+`confirm_bulk_upload` over a temp DB — mixed sheet (two groups + blank-group row + title dup + deadline dup) partitions **4 new / 2 ignored** (title+deadline reasons asserted individually), **2 groups + 2 projects** auto-created, same-group+project rows share one `project_id`, blank-group row `project_id` NULL. Real openpyxl fixture through calamine also **4/2**. 81 app-lib tests green. GUI layer deferred to step 1 (computer-use unavailable in scheduled runs).

- [ ] **3 — nudge-draft LLM title/time pass** | **Opus** | ~1d | **PARKED (2026-07-17)**
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
- [x] **11d** — Bulk Upload P4: `validate_bulk_upload`/`confirm_bulk_upload` tauri commands + `Store::insert_bulk` one-tx resolve+insert w/ `project_id`; 80 app-lib tests green (session 67).
- [x] **11e** — Bulk Upload P5: `BulkUpload.svelte` two-table editable UI (report block + valid) w/ server-driven bidirectional row movement, Group/Project columns, `.csv`/`.xlsx` load; api.js wrappers + `bulkUploadConfirm`; Import rail tab → Tasks-page sub-view; build green (session 68).
- [x] **11f** — Bulk Upload P6: verify — fresh release build + permanent `mixed_xlsx_bulk_upload_end_to_end` test (4 new / 2 ignored, 2 groups + 2 projects, shared `project_id`, blank-group NULL); real openpyxl fixture via calamine 4/2; 81 app-lib tests green. **Step 11 (Bulk Upload) complete.** GUI drive deferred (scheduled-run computer-use block) → step 1 (session 69).

- [x] **2 (§7.2 Svelte surfacing)** — edit form → `update_task`: `api.js updateTask` + `store saveTask`; `TaskPage.svelte` ✎ Edit inline form (prefilled, Deadline-derived minutes, adopts returned DTO binding) + Calendar-event bound/— in detail; build green 132 modules (session 72).
- [x] **2 (§7.2 Delete Event)** — `delete_event(event_id)` command (push `google::calendar::delete_event` first, then `Store::delete_event` + `clear_tasks_bound_to_event`); `api.js deleteEvent` + `Calendar.svelte` edit-dialog Delete button; 85 app-lib tests green, build green (session 73).

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
