//! CSV bulk-task import — P1 pure parser (PLAN-csv-import.md §3).
//!
//! `parse_import(text)` turns a CSV blob into one [`ImportRow`] per data record,
//! each carrying a ready-to-insert [`NewTaskForm`] plus a validity verdict and
//! per-field error list for the filter screen. No I/O, no DB, no clock beyond the
//! machine timezone used to anchor a `deadline` string to unix seconds — so the
//! full parse matrix is unit-testable off-line.
//!
//! Contract mirrors `add_task` (lib.rs): rows become `NewTaskForm` → `Task` →
//! `Db::insert`. `task_source` is forced `Manual` downstream; `gcal_event_id`
//! and `logged_minutes` are not importable. Header row is order-independent,
//! case-insensitive, with a fixed alias map; extra columns are ignored. An
//! unmappable/invalid row is *shown invalid*, never silently coerced.

use crate::NewTaskForm;
use chrono::{Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};
use nudge_core::rules::parse_hhmm;
use nudge_core::tasks::Recur;
use serde::Serialize;

/// Canonical field → accepted header aliases (all matched lower-cased + trimmed).
/// The first entry of each row is the canonical name written by the template.
const ALIASES: &[(&str, &[&str])] = &[
    ("project_group", &["project_group", "group"]),
    ("project", &["project"]),
    ("title", &["title"]),
    ("description", &["description", "desc"]),
    ("deadline", &["deadline", "due"]),
    ("time_of_day", &["time_of_day", "time"]),
    ("recur", &["recur", "repeat"]),
    ("task_type", &["task_type", "type"]),
    ("estimate_minutes", &["estimate_minutes", "estimate"]),
    ("mode", &["mode"]),
];

/// One parsed CSV record. `form` is always populated (best-effort); `valid`
/// gates whether it may be imported, with `errors` explaining any rejection.
/// `raw` preserves the original (canonical-field, cell) pairs so the UI can show
/// and inline-edit exactly what the user supplied.
pub struct ImportRow {
    pub form: NewTaskForm,
    /// Resolved-on-confirm Project Group name (empty ⇒ unfiled). Blank group with
    /// a non-blank project is a row error (a project needs a group — PLAN §3).
    pub project_group: String,
    /// Project name within `project_group` (empty ⇒ unfiled under group, or fully
    /// unfiled when the group is also empty).
    pub project: String,
    pub raw: Vec<(String, String)>,
    pub valid: bool,
    pub errors: Vec<String>,
}

/// Wire shape of one parsed record for the filter screen (P2). Carries the
/// best-effort `form` (so the UI can render/edit mapped cells), the original
/// (field, cell) pairs, and the validity verdict + per-field errors.
#[derive(Serialize)]
pub struct ImportRowDto {
    pub form: NewTaskForm,
    pub project_group: String,
    pub project: String,
    pub raw: Vec<(String, String)>,
    pub valid: bool,
    pub errors: Vec<String>,
}

impl From<ImportRow> for ImportRowDto {
    fn from(r: ImportRow) -> Self {
        ImportRowDto {
            form: r.form,
            project_group: r.project_group,
            project: r.project,
            raw: r.raw,
            valid: r.valid,
            errors: r.errors,
        }
    }
}

/// Resolve a header cell to its canonical field name, if recognized.
fn canonical_of(header: &str) -> Option<&'static str> {
    let h = header.trim().to_ascii_lowercase();
    for (canon, aliases) in ALIASES {
        if aliases.iter().any(|a| *a == h) {
            return Some(canon);
        }
    }
    None
}

/// Anchor a naive local wall-clock instant to unix seconds in the machine tz.
/// Ambiguous (DST fold) or non-existent (spring-forward gap) instants are a
/// per-row error rather than a silent guess.
fn to_local_unix(dt: NaiveDateTime) -> Result<i64, String> {
    match Local.from_local_datetime(&dt).single() {
        Some(local) => Ok(local.timestamp()),
        None => Err(format!(
            "ambiguous or invalid local time '{}'",
            dt.format("%Y-%m-%dT%H:%M")
        )),
    }
}

/// Parse a `deadline` cell (`YYYY-MM-DD` or `YYYY-MM-DDTHH:MM`) to
/// `(unix_seconds, minutes_of_day)` where the minutes are `Some` only when the
/// string carried an explicit time component.
fn parse_deadline(s: &str) -> Result<(i64, Option<u32>), String> {
    let s = s.trim();
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
        let unix = to_local_unix(dt)?;
        return Ok((unix, Some(dt.hour() * 60 + dt.minute())));
    }
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = date.and_hms_opt(0, 0, 0).expect("00:00:00 is valid");
        let unix = to_local_unix(dt)?;
        return Ok((unix, None));
    }
    Err(format!(
        "bad deadline '{s}' (expected YYYY-MM-DD or YYYY-MM-DDTHH:MM)"
    ))
}

/// File-format seam for bulk upload (PLAN-bulk-upload.md §4). Turns raw uploaded
/// bytes into the CSV text that [`parse_import`] consumes. `.csv` bytes pass
/// through as UTF-8 (BOM tolerated downstream); `.xlsx`/`.xlsm` are read in-app
/// with `calamine` (pure-Rust, no runtime/network) — first worksheet, first row =
/// header, cells → strings, re-emitted as canonical CSV. Everything below this
/// seam (header mapping, validation, dedup, hierarchy) is format-agnostic.
pub fn read_spreadsheet(bytes: &[u8], ext: &str) -> Result<String, String> {
    match ext.trim().trim_start_matches('.').to_ascii_lowercase().as_str() {
        "csv" => String::from_utf8(bytes.to_vec())
            .map_err(|_| "file is not valid UTF-8 text".to_string()),
        "xlsx" | "xlsm" => xlsx_to_csv(bytes),
        other => Err(format!("unsupported file type '.{other}' (expected .csv or .xlsx)")),
    }
}

/// Read the first worksheet of an `.xlsx`/`.xlsm` blob and re-serialize it as CSV
/// text (properly quoted) so the single [`parse_import`] path handles it. Cells
/// become strings; blanks stay empty; whole-number floats drop their `.0` so an
/// integer estimate like `90` round-trips as `90`, not `90.0`.
fn xlsx_to_csv(bytes: &[u8]) -> Result<String, String> {
    use calamine::{Data, Reader, Xlsx};
    use std::io::Cursor;

    let mut wb: Xlsx<_> =
        Xlsx::new(Cursor::new(bytes)).map_err(|e| format!("cannot read .xlsx: {e}"))?;
    let sheet = wb
        .worksheet_range_at(0)
        .ok_or_else(|| "spreadsheet has no worksheets".to_string())?
        .map_err(|e| format!("cannot read first worksheet: {e}"))?;

    let mut wtr = csv::Writer::from_writer(Vec::new());
    for row in sheet.rows() {
        let cells: Vec<String> = row
            .iter()
            .map(|c| match c {
                Data::Empty => String::new(),
                Data::String(s) => s.clone(),
                Data::Int(i) => i.to_string(),
                Data::Float(f) if f.fract() == 0.0 => (*f as i64).to_string(),
                Data::Float(f) => f.to_string(),
                Data::Bool(b) => b.to_string(),
                other => other.to_string(),
            })
            .collect();
        wtr.write_record(&cells)
            .map_err(|e| format!("cannot serialize worksheet: {e}"))?;
    }
    let bytes = wtr.into_inner().map_err(|e| format!("csv flush failed: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("csv encode failed: {e}"))
}

/// Parse a CSV blob into per-record [`ImportRow`]s. Blank records are skipped.
/// A missing `title` column makes every row invalid (no field to key on) rather
/// than erroring the whole parse.
pub fn parse_import(text: &str) -> Vec<ImportRow> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes());

    // Map header positions → canonical field names (unknown columns dropped).
    let headers: Vec<Option<&'static str>> = match reader.headers() {
        Ok(h) => h.iter().map(canonical_of).collect(),
        Err(_) => return Vec::new(),
    };
    let has_title = headers.iter().any(|h| *h == Some("title"));

    let mut out = Vec::new();
    for record in reader.records() {
        let record = match record {
            Ok(r) => r,
            Err(e) => {
                out.push(broken_row(format!("unreadable CSV row: {e}")));
                continue;
            }
        };

        // Collapse the record into canonical field → value, preserving order.
        let mut fields: Vec<(String, String)> = Vec::new();
        for (i, cell) in record.iter().enumerate() {
            if let Some(Some(canon)) = headers.get(i) {
                fields.push((canon.to_string(), cell.trim().to_string()));
            }
        }

        // Skip wholly blank records (all mapped cells empty).
        if fields.iter().all(|(_, v)| v.is_empty()) {
            continue;
        }

        out.push(build_row(fields, has_title));
    }
    out
}

/// Lookup a canonical field's value (first occurrence) in the collapsed record.
fn field<'a>(fields: &'a [(String, String)], name: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}

fn build_row(fields: Vec<(String, String)>, has_title: bool) -> ImportRow {
    let mut errors: Vec<String> = Vec::new();

    // title (required, non-empty).
    let title = field(&fields, "title").unwrap_or("").to_string();
    if !has_title {
        errors.push("no 'title' column in header".to_string());
    } else if title.is_empty() {
        errors.push("title is empty".to_string());
    }

    // project_group / project (optional, resolved on confirm). A project without
    // its group is a row error — the hierarchy needs a group to file it under.
    let project_group = field(&fields, "project_group").unwrap_or("").to_string();
    let project = field(&fields, "project").unwrap_or("").to_string();
    if !project.is_empty() && project_group.is_empty() {
        errors.push("project has no project_group".to_string());
    }

    let desc = field(&fields, "description").unwrap_or("").to_string();
    let task_type = field(&fields, "task_type").unwrap_or("").to_string();

    // deadline (optional) → unix + optional derived minutes.
    let mut deadline: Option<i64> = None;
    let mut deadline_minutes: Option<u32> = None;
    if let Some(raw) = field(&fields, "deadline").filter(|s| !s.is_empty()) {
        match parse_deadline(raw) {
            Ok((unix, mins)) => {
                deadline = Some(unix);
                deadline_minutes = mins;
            }
            Err(e) => errors.push(e),
        }
    }

    // time_of_day (optional); else derive from a deadline that carried a time.
    let minutes: Option<u32> = match field(&fields, "time_of_day").filter(|s| !s.is_empty()) {
        Some(raw) => match parse_hhmm(raw) {
            Ok(m) => Some(m),
            Err(_) => {
                errors.push(format!("bad time_of_day '{raw}' (expected HH:MM)"));
                None
            }
        },
        None => deadline_minutes,
    };

    // recur (optional) → validate + normalize to a canonical spec string.
    let recur = match field(&fields, "recur").filter(|s| !s.is_empty()) {
        Some(raw) => match Recur::parse(raw) {
            Ok(r) => r.to_spec(),
            Err(e) => {
                errors.push(e.to_string());
                "once".to_string()
            }
        },
        None => "once".to_string(),
    };

    // estimate_minutes (optional integer).
    let estimate_minutes: Option<u32> = match field(&fields, "estimate_minutes")
        .filter(|s| !s.is_empty())
    {
        Some(raw) => match raw.parse::<u32>() {
            Ok(n) => Some(n),
            Err(_) => {
                errors.push(format!("bad estimate_minutes '{raw}' (expected integer)"));
                None
            }
        },
        None => None,
    };

    // mode (optional); unknown values classify at the edge (None), not an error.
    let mode_override = match field(&fields, "mode").map(str::to_ascii_lowercase).as_deref() {
        Some("off_task") => Some("off_task".to_string()),
        Some("on_task") => Some("on_task".to_string()),
        _ => None,
    };

    let form = NewTaskForm {
        title,
        desc,
        deadline,
        task_type,
        minutes,
        recur,
        mode_override,
        estimate_minutes,
    };

    ImportRow {
        form,
        project_group,
        project,
        raw: fields,
        valid: errors.is_empty(),
        errors,
    }
}

/// An unparseable record we still surface (invalid) so the count matches input.
fn broken_row(msg: String) -> ImportRow {
    ImportRow {
        form: NewTaskForm {
            title: String::new(),
            desc: String::new(),
            deadline: None,
            task_type: String::new(),
            minutes: None,
            recur: "once".to_string(),
            mode_override: None,
            estimate_minutes: None,
        },
        project_group: String::new(),
        project: String::new(),
        raw: Vec::new(),
        valid: false,
        errors: vec![msg],
    }
}

// ---------------------------------------------------------------------------
// Dedup (PLAN-bulk-upload.md §5) — pure over an injected existing-key set.
// ---------------------------------------------------------------------------

use std::collections::HashSet;

/// Normalize a title to its dedup key: trimmed + case-folded (NOCASE).
fn title_key(title: &str) -> String {
    title.trim().to_ascii_lowercase()
}

/// The set of title/deadline keys already present, against which incoming rows
/// are deduped. Built by the caller from a live snapshot of **active** tasks and
/// grown in place as sheet rows are accepted (so intra-sheet dupes collapse).
#[derive(Default)]
pub struct ExistingKeys {
    titles: HashSet<String>,
    /// `None` represents a deadline-less task; it only collides with other
    /// deadline-less rows.
    deadlines: HashSet<Option<i64>>,
}

impl ExistingKeys {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reserve a task's title + deadline so later rows collide against it.
    pub fn insert(&mut self, title: &str, deadline: Option<i64>) {
        self.titles.insert(title_key(title));
        self.deadlines.insert(deadline);
    }
}

/// One row dropped by dedup, paired with the human-readable matched reason.
pub struct IgnoredRow {
    pub row: ImportRow,
    pub reason: String,
}

/// Result of [`dedup_rows`]: rows to show in the valid table vs. the report block.
pub struct DedupOutcome {
    pub new_rows: Vec<ImportRow>,
    pub ignored: Vec<IgnoredRow>,
}

/// Partition parsed rows into new-unique vs. ignored-duplicate (§5).
///
/// A **valid** row is unique iff **both** its title AND deadline are unused; if
/// either already exists (in `existing`, or an earlier accepted sheet row) the
/// row is ignored with the matched reason. Accepted rows reserve their keys so
/// intra-sheet dupes collapse (first occurrence wins). Invalid rows bypass dedup
/// entirely — they flow to `new_rows` to be shown invalid, and never reserve keys.
pub fn dedup_rows(rows: Vec<ImportRow>, existing: &mut ExistingKeys) -> DedupOutcome {
    let mut new_rows = Vec::new();
    let mut ignored = Vec::new();

    for row in rows {
        if !row.valid {
            new_rows.push(row);
            continue;
        }

        let tkey = title_key(&row.form.title);
        let dkey = row.form.deadline;
        let title_hit = existing.titles.contains(&tkey);
        let deadline_hit = existing.deadlines.contains(&dkey);

        if title_hit {
            let reason = format!("title \"{}\" already exists", row.form.title.trim());
            ignored.push(IgnoredRow { row, reason });
        } else if deadline_hit {
            let reason = match dkey {
                Some(ts) => format!("deadline {ts} already exists"),
                None => "empty deadline already exists".to_string(),
            };
            ignored.push(IgnoredRow { row, reason });
        } else {
            existing.titles.insert(tkey);
            existing.deadlines.insert(dkey);
            new_rows.push(row);
        }
    }

    DedupOutcome { new_rows, ignored }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str =
        "project_group,project,title,description,deadline,time_of_day,recur,task_type,estimate_minutes,mode";

    fn parse(rows: &str) -> Vec<ImportRow> {
        parse_import(&format!("{HEADER}\n{rows}"))
    }

    #[test]
    fn full_valid_row() {
        let rows = parse(",,Ship report,Q3 numbers,2026-08-01T14:30,,mon wed fri,work,90,off_task");
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert!(r.valid, "errors: {:?}", r.errors);
        assert_eq!(r.form.title, "Ship report");
        assert_eq!(r.form.desc, "Q3 numbers");
        assert_eq!(r.form.recur, "mon,wed,fri");
        assert_eq!(r.form.task_type, "work");
        assert_eq!(r.form.estimate_minutes, Some(90));
        assert_eq!(r.form.mode_override.as_deref(), Some("off_task"));
        // No explicit time_of_day → derived from the deadline's 14:30.
        assert_eq!(r.form.minutes, Some(14 * 60 + 30));
        assert!(r.form.deadline.is_some());
    }

    #[test]
    fn explicit_time_of_day_wins_over_deadline_time() {
        let rows = parse(",,x,,2026-08-01T14:30,09:15,,,,");
        assert!(rows[0].valid);
        assert_eq!(rows[0].form.minutes, Some(9 * 60 + 15));
    }

    #[test]
    fn date_only_deadline_leaves_minutes_none() {
        let rows = parse(",,x,,2026-08-01,,,,,");
        assert!(rows[0].valid);
        assert!(rows[0].form.deadline.is_some());
        assert_eq!(rows[0].form.minutes, None);
    }

    #[test]
    fn missing_title_is_invalid() {
        let rows = parse(",,,no title,,,,,,");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].valid);
        assert!(rows[0].errors.iter().any(|e| e.contains("title")));
    }

    #[test]
    fn bad_date_time_recur_estimate_each_error() {
        let rows = parse(",,x,,2026-13-01,25:00,funday,,notanint,");
        let r = &rows[0];
        assert!(!r.valid);
        assert_eq!(r.errors.len(), 4, "errors: {:?}", r.errors);
    }

    #[test]
    fn unknown_mode_is_none_not_error() {
        let rows = parse(",,x,,,,,,,bogus");
        assert!(rows[0].valid);
        assert_eq!(rows[0].form.mode_override, None);
    }

    #[test]
    fn blank_rows_skipped() {
        let rows = parse(",,a,,,,,,,\n,,,,,,,,,\n   ,,,,,,,,,\n,,b,,,,,,,");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].form.title, "a");
        assert_eq!(rows[1].form.title, "b");
    }

    #[test]
    fn extra_columns_ignored_order_independent_case_insensitive() {
        let text = "TITLE,Extra,Recur,DESC\ngo,junk,weekdays,note";
        let rows = parse_import(text);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].valid);
        assert_eq!(rows[0].form.title, "go");
        assert_eq!(rows[0].form.desc, "note");
        assert_eq!(rows[0].form.recur, "mon,tue,wed,thu,fri");
    }

    #[test]
    fn quoted_comma_and_bom_handled() {
        // Leading UTF-8 BOM + a quoted field containing a comma.
        let text = "\u{feff}title,description\n\"Call, then email\",\"a, b, c\"";
        let rows = parse_import(text);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].valid, "errors: {:?}", rows[0].errors);
        assert_eq!(rows[0].form.title, "Call, then email");
        assert_eq!(rows[0].form.desc, "a, b, c");
    }

    #[test]
    fn missing_title_column_marks_all_rows_invalid() {
        let text = "description,recur\nhello,daily";
        let rows = parse_import(text);
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].valid);
        assert!(rows[0].errors.iter().any(|e| e.contains("title")));
    }

    #[test]
    fn aliases_due_and_type_and_estimate() {
        let text = "title,due,type,estimate\nx,2026-08-01,errand,15";
        let rows = parse_import(text);
        assert!(rows[0].valid, "errors: {:?}", rows[0].errors);
        assert!(rows[0].form.deadline.is_some());
        assert_eq!(rows[0].form.task_type, "errand");
        assert_eq!(rows[0].form.estimate_minutes, Some(15));
    }

    // --- P2: project_group / project columns --------------------------------

    #[test]
    fn group_and_project_captured() {
        let rows = parse("Marketing,Q3 Launch,Draft brief,,,,,,,");
        let r = &rows[0];
        assert!(r.valid, "errors: {:?}", r.errors);
        assert_eq!(r.project_group, "Marketing");
        assert_eq!(r.project, "Q3 Launch");
        assert_eq!(r.form.title, "Draft brief");
    }

    #[test]
    fn group_without_project_is_valid_unfiled_project() {
        let rows = parse("Marketing,,Draft brief,,,,,,,");
        assert!(rows[0].valid, "errors: {:?}", rows[0].errors);
        assert_eq!(rows[0].project_group, "Marketing");
        assert_eq!(rows[0].project, "");
    }

    #[test]
    fn project_without_group_is_error() {
        let rows = parse(",Q3 Launch,Draft brief,,,,,,,");
        let r = &rows[0];
        assert!(!r.valid);
        assert!(r.errors.iter().any(|e| e.contains("project_group")));
    }

    #[test]
    fn group_alias_group() {
        let text = "group,project,title\nOps,Migration,Backup DB";
        let rows = parse_import(text);
        assert!(rows[0].valid, "errors: {:?}", rows[0].errors);
        assert_eq!(rows[0].project_group, "Ops");
        assert_eq!(rows[0].project, "Migration");
    }

    // --- P2: dedup ----------------------------------------------------------

    /// Build a valid ImportRow with the given title/deadline for dedup tests.
    fn row(title: &str, deadline: Option<i64>) -> ImportRow {
        let rows = parse_import(&format!(
            "title,deadline\n{title},{}",
            deadline
                .map(|_| "2026-08-01T00:00")
                .unwrap_or("")
        ));
        let mut r = rows.into_iter().next().unwrap();
        // Override with the exact injected deadline (parse gives a real unix ts).
        r.form.deadline = deadline;
        assert!(r.valid, "seed row invalid: {:?}", r.errors);
        r
    }

    #[test]
    fn dedup_unique_row_passes() {
        let mut ex = ExistingKeys::new();
        ex.insert("Existing", Some(100));
        let out = dedup_rows(vec![row("Fresh", Some(200))], &mut ex);
        assert_eq!(out.new_rows.len(), 1);
        assert!(out.ignored.is_empty());
    }

    #[test]
    fn dedup_title_collision_ignored() {
        let mut ex = ExistingKeys::new();
        ex.insert("Report", Some(100));
        // Different deadline, same title (NOCASE) → ignored on title.
        let out = dedup_rows(vec![row("report", Some(999))], &mut ex);
        assert!(out.new_rows.is_empty());
        assert_eq!(out.ignored.len(), 1);
        assert!(out.ignored[0].reason.contains("title"));
    }

    #[test]
    fn dedup_deadline_collision_ignored() {
        let mut ex = ExistingKeys::new();
        ex.insert("Report", Some(555));
        let out = dedup_rows(vec![row("Totally new", Some(555))], &mut ex);
        assert!(out.new_rows.is_empty());
        assert_eq!(out.ignored.len(), 1);
        assert!(out.ignored[0].reason.contains("deadline"));
    }

    #[test]
    fn dedup_empty_deadline_only_collides_with_empty() {
        let mut ex = ExistingKeys::new();
        ex.insert("Has empty", None);
        // New title, empty deadline → collides with the existing empty deadline.
        let out = dedup_rows(vec![row("Brand new", None)], &mut ex);
        assert!(out.new_rows.is_empty());
        assert_eq!(out.ignored.len(), 1);
    }

    #[test]
    fn dedup_both_empty_first_wins_intra_sheet() {
        let mut ex = ExistingKeys::new();
        let out = dedup_rows(
            vec![row("Alpha", None), row("Beta", None)],
            &mut ex,
        );
        // First deadline-less row accepted; second collides on empty deadline.
        assert_eq!(out.new_rows.len(), 1);
        assert_eq!(out.new_rows[0].form.title, "Alpha");
        assert_eq!(out.ignored.len(), 1);
        assert_eq!(out.ignored[0].row.form.title, "Beta");
    }

    #[test]
    fn dedup_intra_sheet_title_dupe_reported() {
        let mut ex = ExistingKeys::new();
        let out = dedup_rows(
            vec![row("Same", Some(1)), row("same", Some(2))],
            &mut ex,
        );
        assert_eq!(out.new_rows.len(), 1);
        assert_eq!(out.ignored.len(), 1);
        assert!(out.ignored[0].reason.contains("title"));
    }

    // --- P3: .xlsx reader ---------------------------------------------------

    #[test]
    fn read_spreadsheet_csv_passthrough_and_bad_ext() {
        let csv = "title,recur\ngo,daily";
        assert_eq!(read_spreadsheet(csv.as_bytes(), "csv").unwrap(), csv);
        assert_eq!(read_spreadsheet(csv.as_bytes(), ".CSV").unwrap(), csv);
        assert!(read_spreadsheet(csv.as_bytes(), "txt").is_err());
    }

    #[test]
    fn read_spreadsheet_xlsx_feeds_parse_import() {
        use rust_xlsxwriter::Workbook;

        let mut wb = Workbook::new();
        let ws = wb.add_worksheet();
        // Header + one valid row; estimate written as a number to prove the
        // whole-float → integer path (90.0 must round-trip as "90", not "90.0").
        let header = [
            "project_group",
            "project",
            "title",
            "description",
            "deadline",
            "time_of_day",
            "recur",
            "task_type",
            "estimate_minutes",
            "mode",
        ];
        for (c, h) in header.iter().enumerate() {
            ws.write_string(0, c as u16, *h).unwrap();
        }
        ws.write_string(1, 0, "Marketing").unwrap();
        ws.write_string(1, 1, "Q3 Launch").unwrap();
        ws.write_string(1, 2, "Ship report").unwrap();
        ws.write_string(1, 4, "2026-08-01").unwrap();
        ws.write_number(1, 8, 90.0).unwrap();
        let bytes = wb.save_to_buffer().unwrap();

        let csv = read_spreadsheet(&bytes, "xlsx").unwrap();
        let rows = parse_import(&csv);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert!(r.valid, "errors: {:?}", r.errors);
        assert_eq!(r.project_group, "Marketing");
        assert_eq!(r.project, "Q3 Launch");
        assert_eq!(r.form.title, "Ship report");
        assert!(r.form.deadline.is_some());
        assert_eq!(r.form.estimate_minutes, Some(90));
    }

    #[test]
    fn dedup_invalid_rows_bypass_and_dont_reserve_keys() {
        let mut ex = ExistingKeys::new();
        // An invalid row (project without group) with title "Dup".
        let invalid = {
            let mut r = parse(",Proj,Dup,,,,,,,").into_iter().next().unwrap();
            assert!(!r.valid);
            r.form.deadline = Some(7);
            r
        };
        let valid = row("Dup", Some(7));
        let out = dedup_rows(vec![invalid, valid], &mut ex);
        // Invalid row flows to new_rows shown-invalid; it did NOT reserve "Dup",
        // so the following valid "Dup" is accepted (not reported a collision).
        assert_eq!(out.new_rows.len(), 2);
        assert!(out.ignored.is_empty());
    }
}
