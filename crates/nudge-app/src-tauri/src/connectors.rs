//! Connector ingest (10e): turn upcoming Google Calendar events and actionable
//! Gmail messages into `suggested_triggers` rows, deduped against the tasks a
//! user already has and the suggestions already sitting in their inbox. A human
//! still Accepts/Dismisses each — this only fills the inbox, it never lands a
//! live trigger (GOOGLE-PLAN.md §DB additions: connector guess ≠ live task).
//!
//! Runs synchronously, piggybacking `refresh_calendars` (calendar side reads
//! the cache that refresh just repopulated; Gmail side is a fresh read-only
//! pull). The classification/phrasing helpers are pure and unit-tested; the one
//! network dependency (Gmail) is injected as already-fetched candidates in
//! tests. An optional nudge-draft LLM pass to sharpen titles / infer times is
//! deferred (see NEXTSTEPS / HISTORY) — the deterministic heuristics below are
//! the shipped floor and keep the connector offline-diagnosable.

use crate::db::Store;
use crate::google::gmail::{self, EmailCandidate};

/// How far ahead a calendar event is worth nudging about. Beyond this it isn't
/// actionable yet and would just clutter the inbox.
const GCAL_HORIZON_SECS: i64 = 14 * 24 * 3600;
/// Gmail search: recent inbox mail only. The keyword heuristic below narrows
/// further; this just bounds the fetch.
const GMAIL_QUERY: &str = "in:inbox newer_than:7d";
const GMAIL_MAX_RESULTS: u32 = 25;

/// Words that mark an email as a probable commitment/deadline rather than
/// newsletter/receipt noise. Matched case-insensitively against subject+snippet.
const ACTION_KEYWORDS: &[&str] = &[
    "due", "deadline", "rsvp", "respond", "reply", "submit", "review", "sign",
    "action required", "please", "reminder", "follow up", "follow-up",
    "confirm", "approve", "complete", "by eod", "by end of", "expires",
];

/// What a connector run deposited, surfaced to the caller/UI as a toast.
#[derive(Default)]
pub struct ConnectorSummary {
    pub gcal_added: usize,
    pub gmail_added: usize,
    pub gmail_scanned: usize,
}

/// Deposit fresh suggestions from both connectors. `now` is unix seconds
/// (injected so the pure window math stays testable). Calendar candidates come
/// from the local `cal_events` cache (already refreshed); Gmail candidates are
/// pulled live via `access_token`. A Gmail failure is non-fatal: the calendar
/// side still deposits and the error is folded into the returned message-free
/// summary path (surfaced as `Err` only if it fails before any deposit).
pub fn run_connectors(
    store: &Store,
    access_token: &str,
    now: i64,
) -> Result<ConnectorSummary, String> {
    let mut summary = ConnectorSummary::default();

    // --- Calendar: upcoming timed events → suggestions ---
    let mut known_ids = store
        .known_gcal_event_ids()
        .map_err(|e| format!("known gcal ids: {e}"))?;
    let events = store
        .list_events(now, now + GCAL_HORIZON_SECS)
        .map_err(|e| format!("list upcoming events: {e}"))?;
    for ev in events {
        // Skip all-day events (no discrete prep moment) and anything already
        // mirrored as a task or pending suggestion.
        if ev.all_day || ev.start_unix < now || known_ids.contains(&ev.event_id) {
            continue;
        }
        store
            .insert_suggested_trigger(
                &event_title(&ev.summary),
                "",
                Some(ev.start_unix),
                "gcal",
                Some(&ev.event_id),
                now,
            )
            .map_err(|e| format!("insert gcal suggestion: {e}"))?;
        known_ids.insert(ev.event_id); // guard against intra-run dup
        summary.gcal_added += 1;
    }

    // --- Gmail: actionable recent mail → suggestions ---
    let mut pending_titles = store
        .pending_titles_for_source("gmail")
        .map_err(|e| format!("pending gmail titles: {e}"))?;
    let candidates = gmail::list_candidates(access_token, GMAIL_QUERY, GMAIL_MAX_RESULTS)?;
    summary.gmail_scanned = candidates.len();
    for cand in &candidates {
        if !looks_actionable(cand) {
            continue;
        }
        let title = email_title(cand);
        if pending_titles.contains(&title) {
            continue;
        }
        store
            .insert_suggested_trigger(&title, &email_description(cand), None, "gmail", None, now)
            .map_err(|e| format!("insert gmail suggestion: {e}"))?;
        pending_titles.insert(title);
        summary.gmail_added += 1;
    }

    Ok(summary)
}

// --- pure helpers (unit-tested) ---

/// A calendar suggestion's title: the event summary, trimmed, with an empty one
/// backfilled so the inbox never shows a blank row.
fn event_title(summary: &str) -> String {
    let t = summary.trim();
    if t.is_empty() {
        "(untitled event)".to_string()
    } else {
        t.to_string()
    }
}

/// True if an email reads like something the user must act on, by keyword match
/// on subject+snippet. Deliberately lossy — the user still triages the inbox, so
/// a false positive costs one Dismiss, not a bad live trigger.
fn looks_actionable(c: &EmailCandidate) -> bool {
    let hay = format!("{} {}", c.subject, c.snippet).to_lowercase();
    ACTION_KEYWORDS.iter().any(|k| hay.contains(k))
}

/// A Gmail suggestion's title: the subject, stripped of common `Re:`/`Fwd:`
/// prefixes and clamped so a long subject doesn't blow out the inbox row.
fn email_title(c: &EmailCandidate) -> String {
    let mut s = c.subject.trim();
    loop {
        let lower = s.to_lowercase();
        if let Some(rest) = lower
            .strip_prefix("re:")
            .or_else(|| lower.strip_prefix("fwd:"))
            .or_else(|| lower.strip_prefix("fw:"))
        {
            let cut = s.len() - rest.len();
            s = s[cut..].trim();
        } else {
            break;
        }
    }
    let s = if s.is_empty() { "(no subject)" } else { s };
    clamp(s, 120)
}

/// Sender attribution kept in the suggestion description so the user has
/// provenance when triaging ("who is this from?") without opening Gmail.
fn email_description(c: &EmailCandidate) -> String {
    let from = c.from.trim();
    if from.is_empty() {
        String::new()
    } else {
        clamp(&format!("From: {from}"), 200)
    }
}

/// Truncate on a char boundary, appending an ellipsis when cut.
fn clamp(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(subject: &str, snippet: &str, from: &str) -> EmailCandidate {
        EmailCandidate {
            message_id: "m".into(),
            subject: subject.into(),
            from: from.into(),
            snippet: snippet.into(),
            internal_date_unix: 0,
        }
    }

    #[test]
    fn event_title_backfills_blank() {
        assert_eq!(event_title("  "), "(untitled event)");
        assert_eq!(event_title("  Standup "), "Standup");
    }

    #[test]
    fn looks_actionable_matches_keywords() {
        assert!(looks_actionable(&cand("Report due Friday", "", "")));
        assert!(looks_actionable(&cand("Meeting", "Please RSVP by noon", "")));
        assert!(!looks_actionable(&cand("Weekly newsletter", "top stories", "")));
    }

    #[test]
    fn looks_actionable_is_case_insensitive() {
        assert!(looks_actionable(&cand("ACTION REQUIRED", "", "")));
    }

    #[test]
    fn email_title_strips_reply_prefixes() {
        assert_eq!(email_title(&cand("Re: Report due", "", "")), "Report due");
        assert_eq!(email_title(&cand("Fwd: Re: Sign this", "", "")), "Sign this");
        assert_eq!(email_title(&cand("FW: Approve", "", "")), "Approve");
    }

    #[test]
    fn email_title_clamps_long_subject() {
        let long = "x".repeat(200);
        let title = email_title(&cand(&long, "", ""));
        assert_eq!(title.chars().count(), 120);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn email_description_carries_sender() {
        assert_eq!(
            email_description(&cand("s", "", "boss@example.com")),
            "From: boss@example.com"
        );
        assert_eq!(email_description(&cand("s", "", "  ")), "");
    }

    #[test]
    fn clamp_respects_char_boundaries() {
        // Multibyte chars must not be split mid-codepoint.
        let s = "é".repeat(10);
        let out = clamp(&s, 5);
        assert_eq!(out.chars().count(), 5);
    }
}
