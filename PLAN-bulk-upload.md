<!-- Boundary: forward-looking design doc for the Bulk Upload feature (Project Groups → Projects → Tasks hierarchy + spreadsheet ingest). UI + app-process only; nudge-svc untouched. Zero new polling, zero network I/O. -->

# PLAN: Bulk Upload (task hierarchy + spreadsheet ingest)

Supersedes / extends [PLAN-csv-import.md](PLAN-csv-import.md). The CSV import work (step 7, P1–P3 done)
is the substrate: same row → `NewTaskForm` → `Task` → batch-insert contract. This plan adds (a) a
three-level org hierarchy, (b) `.xlsx` ingest, (c) a rename to **Bulk Upload** reached from the Tasks
page, and (d) title/deadline **dedup with an ignored-rows report** ahead of an editable confirm table.

## 1. Problem / Success / Constraints
- **Problem:** A meeting transcript is turned (outside nudge-bot, in Claude/Gemini) into a spreadsheet of
  candidate action items. The user needs to ingest that sheet, file each task under the right Project and
  Project Group, drop duplicates against existing tasks, and review/edit before anything lands.
- **Success:** User opens **Bulk Upload** from the Tasks page, uploads `.csv` or `.xlsx`, sees (1) a
  **report block** listing every row ignored as a duplicate (with the reason), and (2) an editable table of
  the new, unique tasks pre-filed under their Project Group → Project. User edits tentative fields, presses
  **Confirm**, every included row lands in `tasks` under its project in one reload.
- **Load-bearing constraints:**
  - App-process only. `nudge-svc`/`nudge-ctl` and the resource budget untouched — one-shot UI action
    writing `sessions.db`, then the existing single `insert_and_reload` refresh.
  - Reuse the ingestion contract; hierarchy is additive (new tables + one nullable FK on `tasks`).
  - Determinism: group/project matching is exact case-insensitive string match on name, no fuzzy merge;
    dedup rule is mechanical (below); an unmappable row is *shown*, never silently coerced.

## 2. Data model (additive, app-owned — svc never reads these)
Two new tables in `nudge-app/src-tauri/src/db.rs` (no parity copy in `persist.rs`; the svc reads only
`tasks`/`task_tools`). Add one nullable FK to `tasks`.

```sql
CREATE TABLE IF NOT EXISTS project_groups (
    id   INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE IF NOT EXISTS projects (
    id       INTEGER PRIMARY KEY,
    group_id INTEGER NOT NULL REFERENCES project_groups(id),
    name     TEXT NOT NULL COLLATE NOCASE,
    UNIQUE (group_id, name)
);
-- additive, tolerant ALTER (same idiom as the §7.1 task_source rename):
ALTER TABLE tasks ADD COLUMN project_id INTEGER;   -- nullable; unfiled tasks = NULL
```

- A Task references at most one Project (`tasks.project_id`, nullable). A Project belongs to exactly one
  Group. Names unique within scope (Group name globally; Project name within its Group).
- On ingest, a row's **Project Group** and **Project** string columns resolve to ids: exact NOCASE match
  reuses the existing row; no match **creates** it (group first, then project under it). Blank group/project
  ⇒ `project_id = NULL` (unfiled) — allowed.

## 3. Spreadsheet schema (give this to Claude/Gemini)
Canonical CSV header from PLAN-csv-import.md, **plus two leading columns**:

```
project_group,project,title,description,deadline,time_of_day,recur,task_type,estimate_minutes,mode
```

- **project_group** — optional string; names/creates the Project Group.
- **project** — optional string; names/creates the Project under that Group. Requires `project_group`
  present (a project without a group is a row error).
- Remaining columns: unchanged from PLAN-csv-import.md §2. A single sheet may address many groups/projects
  (per-row) or all one project (same value every row) — both fall out of per-row resolution.
- The "Copy prompt for Claude/Gemini" and "Download blank template" buttons emit this 10-column header.

## 4. `.xlsx` ingest
- Parse in-app with the `calamine` crate (pure-Rust, no runtime, no network — budget-safe). Read the first
  worksheet, first row = header, cells → strings, then hand to the **same** `parse_import` path as CSV.
- `.csv` keeps the existing `csv`-crate path. File-type branch is at the read boundary only; validation,
  dedup, hierarchy, and insert are format-agnostic below that. Web-view file pick via `FileReader`
  (`.readAsArrayBuffer` for xlsx) — no tauri fs/dialog plugin, mirrors CsvImport.svelte P3.

## 5. Dedup rule + report block (the new gate)
An incoming row is a **new unique Task** iff **both** its `title` and its `deadline` are absent from the
existing `tasks` set. If **either** the title **or** the deadline already exists on any current task, the
row is **ignored** and listed in the report block with the matched reason (`title "X" already exists` /
`deadline <ts> already exists`). Empty deadline matches only other empty deadlines. Matching is exact:
title NOCASE-trimmed; deadline on the parsed unix value. Intra-sheet duplicates collapse the same way
(first occurrence wins, later ones reported ignored).

Server-side (`validate_bulk_upload`) computes this against a live `list_tasks` snapshot of **active** tasks
(completed/archived history is not consulted) so the report can't drift from the DB; the client never
decides duplication.

The report block is **not** read-only: an ignored row is fully editable in place (same inline cells as the
valid table). Editing re-runs §5 on that single row against the same snapshot — if the edit makes the
row's title **and** deadline both unique (e.g. the user nudges the deadline), the row **moves up to the
valid table**; conversely a valid-table edit that reintroduces a collision moves that row down into the
report block. Movement is bidirectional and driven solely by the mechanical rule, not a manual toggle.

## 6. UI flow (`BulkUpload.svelte`)
Reached from the **Tasks page** (a "Bulk Upload" button/entry, not a top-level rail item — replaces/renames
the standalone Import tab). Flow:
1. Pick `.csv`/`.xlsx` (or paste CSV) → `validate_bulk_upload` returns `{ new_rows, ignored_rows }`.
2. **Report block** (top): ignored rows + matched reason, count badge. **Fully editable** (same inline
   cells as the valid table) — the user can fix a duplicate here (e.g. change the deadline); when the edit
   makes title AND deadline both unique the row **jumps up to the valid table** (§5).
3. **Editable table** (below): one row per new unique task, columns include resolved **Project Group** and
   **Project** (editable text — editing re-resolves/creates on confirm), plus the task fields, inline-
   editable, with a per-row status pill and include toggle (auto-off for invalid). Re-validates the single
   row on edit; an edit that reintroduces a title/deadline collision moves the row **down** to the report
   block. Movement between the two tables is bidirectional and rule-driven, not a manual toggle.
4. **Confirm** (footer, disabled until ≥1 valid+included) → `confirm_bulk_upload(rows)` → single sqlite
   transaction: resolve/create groups+projects, insert tasks with `project_id`, **one** `signal_reload` →
   toast + jump to Tasks/Planner.

## 7. Phases (one per session)
- **P1 — schema + hierarchy resolver (backend, pure/DB).** New tables + `project_id` ALTER (both the
  `db.rs` create block and a tolerant migration). `resolve_group_project(name,name) -> ids` (exact NOCASE
  match-or-create, transaction-safe). Unit-testable off a temp DB. *(Sonnet.)*
- **P2 — parser + dedup (mostly pure).** Extend `parse_import` to the 10-column header (2 new fields).
  Add `dedup_rows(rows, existing) -> (new, ignored)` implementing §5 — pure over an injected existing-set,
  fully unit-testable (title-collision, deadline-collision, both-empty, intra-sheet dupes, blank
  group/project). *(Sonnet.)*
- **P3 — `.xlsx` reader.** `calamine` behind a `read_spreadsheet(bytes, ext)` seam feeding `parse_import`.
  Unit test a tiny fixture `.xlsx`. Keep CSV path untouched. *(Sonnet.)*
- **P4 — tauri commands.** `validate_bulk_upload(text_or_bytes, ext) -> {new_rows, ignored_rows}` and
  `confirm_bulk_upload(rows) -> Result<usize>` (one transaction: resolve hierarchy + `insert_batch` +
  single reload; server-side re-validate + re-dedup, don't trust client toggles). Register in
  `generate_handler!`. *(Sonnet.)*
- **P5 — `BulkUpload.svelte` + Tasks-page entry.** §6 UI: rename Import→Bulk Upload, move entry under the
  Tasks page, report block, editable table with Group/Project columns, Confirm. api.js wrappers +
  store batch helper. *(Opus — cross-cutting UI/state design, hierarchy editing in-table.)*
- **P6 — verify.** Unit matrices green (resolver, dedup, xlsx). Live drive a mixed `.xlsx`: rows across two
  groups + a deliberate title dup + a deliberate deadline dup + one blank-group row → confirm the report
  block flags exactly the dupes, groups/projects auto-create, Confirm writes only included rows with
  correct `project_id`, one reload. *(Sonnet.)*

## 8. Explicitly out of scope
- No in-app transcript→spreadsheet step — that stays in the user's Claude/Gemini chat (no in-app LLM/network).
- No project/group management UI (rename/delete/reparent) in this feature — creation-on-ingest only; a
  dedicated Projects manager is a later step.
- No cross-project task moves, no multi-project tasks (one `project_id`, hard rule).
- No column-remap wizard; fixed header contract (wrong header ⇒ rows shown invalid).
- No svc changes.
