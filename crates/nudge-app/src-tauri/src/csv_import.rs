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

/// Canonical field → accepted header aliases (all matched lower-cased + trimmed).
/// The first entry of each row is the canonical name written by the template.
const ALIASES: &[(&str, &[&str])] = &[
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
    pub raw: Vec<(String, String)>,
    pub valid: bool,
    pub errors: Vec<String>,
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
        raw: Vec::new(),
        valid: false,
        errors: vec![msg],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "title,description,deadline,time_of_day,recur,task_type,estimate_minutes,mode";

    fn parse(rows: &str) -> Vec<ImportRow> {
        parse_import(&format!("{HEADER}\n{rows}"))
    }

    #[test]
    fn full_valid_row() {
        let rows = parse("Ship report,Q3 numbers,2026-08-01T14:30,,mon wed fri,work,90,off_task");
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
        let rows = parse("x,,2026-08-01T14:30,09:15,,,,");
        assert!(rows[0].valid);
        assert_eq!(rows[0].form.minutes, Some(9 * 60 + 15));
    }

    #[test]
    fn date_only_deadline_leaves_minutes_none() {
        let rows = parse("x,,2026-08-01,,,,,");
        assert!(rows[0].valid);
        assert!(rows[0].form.deadline.is_some());
        assert_eq!(rows[0].form.minutes, None);
    }

    #[test]
    fn missing_title_is_invalid() {
        let rows = parse(",no title,,,,,,");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].valid);
        assert!(rows[0].errors.iter().any(|e| e.contains("title")));
    }

    #[test]
    fn bad_date_time_recur_estimate_each_error() {
        let rows = parse("x,,2026-13-01,25:00,funday,,notanint,");
        let r = &rows[0];
        assert!(!r.valid);
        assert_eq!(r.errors.len(), 4, "errors: {:?}", r.errors);
    }

    #[test]
    fn unknown_mode_is_none_not_error() {
        let rows = parse("x,,,,,,,bogus");
        assert!(rows[0].valid);
        assert_eq!(rows[0].form.mode_override, None);
    }

    #[test]
    fn blank_rows_skipped() {
        let rows = parse("a,,,,,,,\n,,,,,,,\n   ,,,,,,,\nb,,,,,,,");
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
}
