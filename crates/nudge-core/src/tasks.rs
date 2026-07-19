//! Task / trigger data model + the low-friction quick-add parser (UI-PLAN §2
//! Triggers tab, §3 data model). Pure and OS-free like the rest of nudge-core:
//! the parser turns one line of user text into a structured trigger; the future
//! nudge-app owns persistence (the `tasks` table) and svc reads it back.
//!
//! Grammar (`text @ time [recur]`):
//!   "gym @ 17:30 mon,wed,fri"   → text "gym", 17:30, weekly Mon/Wed/Fri
//!   "standup @ 09:00 weekdays"  → weekly Mon..Fri
//!   "call mum @ 18:00"          → one-time at 18:00
//! Recur is one of the keywords `daily` / `weekdays` / `weekends`, or an explicit
//! day list (`mon,wed,fri` or space-separated `mon wed fri`); absent → `Once`.

use crate::rules::parse_hhmm;
use crate::{Mode, UnixTime};

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// How often a trigger repeats. Weekday indices are 0 = Mon .. 6 = Sun, sorted
/// and de-duplicated so equal recurrences compare equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recur {
    /// Fires once (no recurrence).
    Once,
    /// Fires on each listed weekday. Never empty (an empty list is a parse error).
    Weekly(Vec<u8>),
}

impl Recur {
    /// Serialize to a stable spec string for the `tasks` table: `Once` → `"once"`,
    /// `Weekly` → a comma-joined day list (`"mon,wed,fri"`). Round-trips through
    /// [`Recur::parse`]. Kept plain (no keyword folding) so persisted rows are
    /// unambiguous regardless of how the user first typed the recurrence.
    pub fn to_spec(&self) -> String {
        match self {
            Recur::Once => "once".to_string(),
            Recur::Weekly(days) => days
                .iter()
                .map(|d| DAYS[*d as usize])
                .collect::<Vec<_>>()
                .join(","),
        }
    }

    /// Parse a recurrence spec (keyword, day list, `once`, or empty → [`Recur::Once`]).
    /// The inverse of [`Recur::to_spec`] and the same grammar the quick-add parser
    /// accepts after the time token.
    pub fn parse(spec: &str) -> Result<Recur, QuickAddError> {
        parse_recur(spec)
    }
}

/// A parsed quick-add line: the task text and when it fires. Time is minutes
/// since local midnight, mirroring [`crate::schedule`]'s window representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickAdd {
    pub text: String,
    /// Minutes since local midnight (0..=1439).
    pub minutes: u32,
    pub recur: Recur,
}

/// Why a quick-add line could not be parsed. Carries a human-readable message the
/// UI can surface inline next to the input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickAddError(pub String);

impl std::fmt::Display for QuickAddError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

fn err(msg: impl Into<String>) -> QuickAddError {
    QuickAddError(msg.into())
}

/// Parse one weekday name into its 0=Mon..6=Sun index (case-insensitive).
fn parse_day(tok: &str) -> Option<u8> {
    let low = tok.to_ascii_lowercase();
    DAYS.iter().position(|d| *d == low).map(|i| i as u8)
}

/// Expand the optional recurrence clause. `None`/empty → [`Recur::Once`]; a
/// keyword or a day list → [`Recur::Weekly`] with sorted, de-duplicated days.
fn parse_recur(spec: &str) -> Result<Recur, QuickAddError> {
    let spec = spec.trim();
    if spec.is_empty() || spec.eq_ignore_ascii_case("once") {
        return Ok(Recur::Once);
    }
    let days: Vec<u8> = match spec.to_ascii_lowercase().as_str() {
        "daily" | "everyday" => (0..7).collect(),
        "weekdays" => (0..5).collect(),
        "weekends" => vec![5, 6],
        _ => {
            // Explicit day list: comma- and/or whitespace-separated.
            let mut out = Vec::new();
            for tok in spec.split(|c: char| c == ',' || c.is_whitespace()) {
                if tok.is_empty() {
                    continue;
                }
                let d = parse_day(tok)
                    .ok_or_else(|| err(format!("unknown recurrence or day '{tok}'")))?;
                out.push(d);
            }
            out
        }
    };
    if days.is_empty() {
        return Err(err("empty recurrence"));
    }
    let mut days = days;
    days.sort_unstable();
    days.dedup();
    Ok(Recur::Weekly(days))
}

/// Parse a quick-add line `text @ time [recur]`. Returns a structured
/// [`QuickAdd`] or a message-bearing [`QuickAddError`].
pub fn parse_quickadd(input: &str) -> Result<QuickAdd, QuickAddError> {
    let (text, rest) = input
        .split_once('@')
        .ok_or_else(|| err("expected 'text @ time'; missing '@'"))?;
    let text = text.trim();
    if text.is_empty() {
        return Err(err("task text is empty"));
    }
    let rest = rest.trim();
    // First whitespace-separated token after '@' is the time; the remainder,
    // if any, is the recurrence clause.
    let (time_tok, recur_spec) = match rest.split_once(char::is_whitespace) {
        Some((t, r)) => (t, r),
        None => (rest, ""),
    };
    if time_tok.is_empty() {
        return Err(err("missing time after '@'"));
    }
    let minutes = parse_hhmm(time_tok).map_err(|_| err(format!("bad time '{time_tok}'")))?;
    let recur = parse_recur(recur_spec)?;
    Ok(QuickAdd {
        text: text.to_string(),
        minutes,
        recur,
    })
}

/// Where a trigger came from (UI-PLAN §3 import lane). `Manual` is the only
/// producer today; the connector sources are reserved so the `tasks` schema and
/// the "Suggested triggers" inbox can land before Gmail/GCal wiring exists.
/// Unknown labels on read-back fold to `Manual` rather than failing a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TriggerSource {
    /// Entered by the user (quick-add or the trigger form).
    #[default]
    Manual,
    /// Deposited by the (future) Gmail connector.
    Gmail,
    /// Deposited by the (future) Google Calendar connector.
    Gcal,
}

impl TriggerSource {
    /// Stable label persisted in the `tasks` table.
    pub fn label(&self) -> &'static str {
        match self {
            TriggerSource::Manual => "manual",
            TriggerSource::Gmail => "gmail",
            TriggerSource::Gcal => "gcal",
        }
    }

    /// Inverse of [`TriggerSource::label`]; unrecognized labels fall back to
    /// [`TriggerSource::Manual`] so an unknown source never drops a task.
    pub fn from_label(s: &str) -> TriggerSource {
        match s {
            "gmail" => TriggerSource::Gmail,
            "gcal" => TriggerSource::Gcal,
            _ => TriggerSource::Manual,
        }
    }
}

/// A task as it lives in the `tasks` table (UI-PLAN §3). The nudge-app owns
/// writes; the svc reads it back read-only to build nudge windows. Pure data —
/// no persistence or OS concerns leak into nudge-core. Graduates the old
/// `rules.toml` `[[nudge]]` entries: `minutes`/`recur` are the fire schedule,
/// while `rules.toml` keeps global config (anchor, ladders, snooze defaults).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    /// SQLite rowid; `None` before the row is inserted.
    pub id: Option<i64>,
    pub title: String,
    pub desc: String,
    /// Hard deadline (unix seconds); `None` for a schedule-only task with no
    /// deadline. Deadline-critical re-cue logic (UI-PLAN §1) keys off this.
    pub deadline: Option<UnixTime>,
    /// Free-form task category for the Planner filters. Taxonomy is user-supplied
    /// later, so it stays a string; empty = uncategorized.
    pub task_type: String,
    /// Trigger time-of-day (minutes since local midnight); `None` = deadline-only,
    /// no time-of-day cue.
    pub minutes: Option<u32>,
    pub recur: Recur,
    /// Per-task override of the notification mode; `None` = classify at the edge
    /// from live activity as usual (the default path).
    pub mode_override: Option<Mode>,
    pub task_source: TriggerSource,
    /// Google Calendar event id this task mirrors, if imported; `None` otherwise.
    pub gcal_event_id: Option<String>,
    /// Estimated work in minutes (§6.3); `None` = no estimate (window falls back
    /// to the static 48 h horizon and progress-bands read as not-started).
    pub estimate_minutes: Option<u32>,
    /// Minutes worked so far (§6.3), the `X` in the progress fill. Cache of the
    /// last edge value: the svc alone writes it (the sanctioned `logged_minutes`
    /// exception, PLAN §3), recomputed lazily — never ticked. Defaults 0.
    pub logged_minutes: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recur_spec_round_trips() {
        for r in [
            Recur::Once,
            Recur::Weekly(vec![0, 2, 4]),
            Recur::Weekly(vec![0, 1, 2, 3, 4, 5, 6]),
        ] {
            assert_eq!(Recur::parse(&r.to_spec()).unwrap(), r);
        }
        assert_eq!(Recur::Weekly(vec![0, 2, 4]).to_spec(), "mon,wed,fri");
        assert_eq!(Recur::Once.to_spec(), "once");
    }

    #[test]
    fn task_source_labels_round_trip_unknown_to_manual() {
        for s in [TriggerSource::Manual, TriggerSource::Gmail, TriggerSource::Gcal] {
            assert_eq!(TriggerSource::from_label(s.label()), s);
        }
        assert_eq!(TriggerSource::from_label("slack"), TriggerSource::Manual);
    }

    #[test]
    fn one_time_no_recur() {
        let q = parse_quickadd("call mum @ 18:00").unwrap();
        assert_eq!(q.text, "call mum");
        assert_eq!(q.minutes, 18 * 60);
        assert_eq!(q.recur, Recur::Once);
    }

    #[test]
    fn explicit_day_list_comma_and_space() {
        let a = parse_quickadd("gym @ 17:30 mon,wed,fri").unwrap();
        assert_eq!(a.text, "gym");
        assert_eq!(a.minutes, 17 * 60 + 30);
        assert_eq!(a.recur, Recur::Weekly(vec![0, 2, 4]));
        // Space-separated and out-of-order dedupes/sorts identically.
        let b = parse_quickadd("gym @ 17:30 fri mon wed mon").unwrap();
        assert_eq!(b.recur, Recur::Weekly(vec![0, 2, 4]));
    }

    #[test]
    fn recur_keywords() {
        assert_eq!(
            parse_quickadd("x @ 09:00 daily").unwrap().recur,
            Recur::Weekly(vec![0, 1, 2, 3, 4, 5, 6])
        );
        assert_eq!(
            parse_quickadd("x @ 09:00 weekdays").unwrap().recur,
            Recur::Weekly(vec![0, 1, 2, 3, 4])
        );
        assert_eq!(
            parse_quickadd("x @ 09:00 WEEKENDS").unwrap().recur,
            Recur::Weekly(vec![5, 6])
        );
    }

    #[test]
    fn text_may_contain_spaces_case_insensitive_days() {
        let q = parse_quickadd("deep work block @ 08:00 Mon,Tue").unwrap();
        assert_eq!(q.text, "deep work block");
        assert_eq!(q.recur, Recur::Weekly(vec![0, 1]));
    }

    #[test]
    fn errors() {
        assert!(parse_quickadd("no at sign 09:00").is_err()); // missing '@'
        assert!(parse_quickadd("  @ 09:00").is_err()); // empty text
        assert!(parse_quickadd("x @").is_err()); // missing time
        assert!(parse_quickadd("x @ 25:00").is_err()); // bad time
        assert!(parse_quickadd("x @ 09:00 funday").is_err()); // bad recur token
    }
}
