<!-- Boundary: forward-looking design doc for the CSV bulk-task-import feature. UI + app-process only; nudge-svc untouched. Zero new polling, zero network I/O. -->

# PLAN: CSV bulk task import

## 1. Problem / Success / Constraints
- **Problem:** Adding tasks is one-at-a-time via the Triggers form. User wants to dump a text list → have an LLM (Claude/Gemini) shape it into a spreadsheet → upload it → get all rows as tasks, with a verify/filter screen first.
- **Success:** User uploads a `.csv`, sees a per-row filter screen flagging which rows are import-ready (required fields present/parseable) vs. broken, edits/toggles rows, clicks Import, and every included row lands in `tasks` (one rules reload, not N).
- **Load-bearing constraints:**
  - App-process only. `nudge-svc`/`nudge-ctl` and the resource budget are untouched — import is a one-shot UI action writing `sessions.db`, then the existing `insert_and_reload` refresh path.
  - Reuse the existing ingestion contract: rows become `NewTaskForm` → `Task` → `Db::insert` (the same path `add_task` already uses). No new task shape.
  - Determinism: no fuzzy header guessing beyond a fixed alias map; an unmappable/invalid row is *shown as invalid*, never silently coerced.

## 2. Canonical CSV schema (give this to the LLM)
Header row the import expects (order-independent, case-insensitive; extra columns ignored):

```
title,description,deadline,time_of_day,recur,task_type,estimate_minutes,mode
```

- **title** — required, non-empty.
- **description** — optional free text.
- **deadline** — optional, ISO local `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM`. Parsed in machine local tz → unix seconds.
- **time_of_day** — optional `HH:MM` (24h) → minutes-since-midnight. If omitted but a `deadline` with a time is given, derive from the deadline (mirror `local_minutes_of_day`, db.rs). Deadline-date-only + no time_of_day → `schedule` default (09:00) already handles it (step 4).
- **recur** — optional; `once` (default) or a day list `mon,wed,fri`. Parsed by `Recur::parse` — same validation as the form.
- **task_type** — optional string (Planner filter bucket).
- **estimate_minutes** — optional integer.
- **mode** — optional `on_task` | `off_task`; anything else → `None` (classify at edge).

`trigger_source` is forced to `Manual`; `gcal_event_id`/`logged_minutes` are not importable. A one-line **"Copy prompt for Claude/Gemini"** button on the import screen emits this schema + a "return only CSV with this header row" instruction, so the LLM output maps cleanly. A **"Download blank template"** button (first-class, on the import screen *and* offered before any file is picked) writes a `.csv` containing only the canonical header row — the artifact the user hands to Claude/Gemini to fill.

## 3. Phases (one per session; execution = Sonnet)

- **P1 — core parser (`csv_import.rs`, nudge-app src-tauri).** Pure `parse_import(text: &str) -> Vec<ImportRow>`. Uses the `csv` crate for the split, then a fixed header-alias map + per-field parse (chrono for deadline/time, `Recur::parse` for recur). `ImportRow { form: NewTaskForm, raw: Vec<(String,String)>, valid: bool, errors: Vec<String> }`. No I/O, no DB — fully unit-testable off a clock. Cases: missing title, bad date, bad time, bad recur, unknown/missing headers, blank rows skipped, extra columns ignored, BOM/quoted-comma handling.
- **P2 — tauri commands + batch insert.** `validate_csv_import(text) -> Vec<ImportRowDto>` (wraps P1, serializes status+errors to the UI). `import_tasks(rows: Vec<NewTaskForm>) -> Result<usize,String>` — single sqlite transaction over `Db::insert`, then **one** `reload`/refresh at the end (not per row) so a 50-row import triggers one rules reload, not 50. Register both in `generate_handler!`. Reject any row failing server-side re-validation (don't trust the client toggle blindly).
- **P3 — frontend (`CsvImport.svelte` + Sidebar entry).** File pick via tauri dialog (or paste box) → call `validate_csv_import` → filter table: one row per record, columns = mapped fields, a status pill (ready / needs-fix + error text), inline-editable cells for quick fixes, and an **include** checkbox (auto-unchecked for invalid rows, re-validate on edit). Footer: "Import N ready rows" (disabled until ≥1 valid+included) → `import_tasks` → toast + jump to Planner. Plus the "Copy LLM prompt" button and a "Download template.csv" link.
- **P4 — verify.** Unit tests green (P1 parse matrix). Live drive: a hand-made 5-row CSV (mix of valid + deliberately broken rows) → confirm the filter screen flags the broken ones, an inline fix flips a row to ready, Import writes exactly the included rows to `tasks` (check via `list_tasks` / sqlite), and the svc picks up new windows after the single reload.

## 4. Explicitly out of scope
- No `.xlsx` ingestion — CSV only (user converts/exports to CSV; the LLM emits CSV directly). Revisit only if asked.
- No column re-mapping UI — the header contract is fixed; a wrong header shows rows as invalid rather than offering a mapping wizard.
- No svc changes, no connector/LLM calls in-app (the LLM shaping happens outside nudge-bot, in the user's Claude/Gemini chat).
