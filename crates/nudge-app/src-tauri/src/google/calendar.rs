//! Google Calendar REST: `calendarList.list` + `events.list` (read, 10b),
//! `events.insert`/`events.update` (write, 10c — primary calendar only, the
//! caller in `lib.rs` enforces that restriction). Blocking `ureq`, same idiom
//! as `oauth.rs`/`nudge-draft/anthropic.rs`. JSON parsing/building is split
//! into pure, unit-tested helpers; the network fns are thin wrappers (mirrors
//! the `anthropic.rs` `build_body`/`extract_text` split).

use serde_json::Value;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(20);
const CALENDAR_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";

/// One calendar from the user's GCal account.
pub struct CalendarInfo {
    pub gcal_id: String,
    pub summary: String,
    pub bg_color: String,
    pub is_primary: bool,
}

/// One event, already resolved to unix-second bounds (all-day events span
/// local midnight to local midnight, `all_day = true`).
pub struct EventInfo {
    pub event_id: String,
    pub summary: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub all_day: bool,
    pub updated_unix: i64,
    pub etag: String,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(TIMEOUT)
        .timeout_read(TIMEOUT)
        .build()
}

/// `GET /users/me/calendarList` — every calendar on the account (owned +
/// subscribed), unfiltered. Selection/primary-for-write is app-local state
/// layered on top (see `db::Store::upsert_calendars`).
pub fn list_calendars(access_token: &str) -> Result<Vec<CalendarInfo>, String> {
    let resp = agent()
        .get(CALENDAR_LIST_URL)
        .set("Authorization", &format!("Bearer {access_token}"))
        .call()
        .map_err(|e| format!("calendarList request failed: {e}"))?;
    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad calendarList response: {e}"))?;
    Ok(parse_calendar_list(&json))
}

/// `GET /calendars/{id}/events` in `[time_min_unix, time_max_unix)`, expanded
/// (`singleEvents=true`) so recurring events surface as concrete instances
/// rather than one master row.
pub fn list_events(
    access_token: &str,
    calendar_id: &str,
    time_min_unix: i64,
    time_max_unix: i64,
) -> Result<Vec<EventInfo>, String> {
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events",
        path_encode(calendar_id)
    );
    let resp = agent()
        .get(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .query("timeMin", &unix_to_rfc3339(time_min_unix))
        .query("timeMax", &unix_to_rfc3339(time_max_unix))
        .query("singleEvents", "true")
        .query("orderBy", "startTime")
        .query("maxResults", "2500")
        .call()
        .map_err(|e| format!("events.list request failed ({calendar_id}): {e}"))?;
    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad events.list response: {e}"))?;
    Ok(parse_events(&json))
}

/// `POST /calendars/{id}/events` — create a new event. Caller (`lib.rs`)
/// restricts `calendar_id` to the app's chosen primary calendar (GOOGLE-PLAN.md:
/// "write path: create/edit events on ONE designated primary Google Calendar
/// only").
pub fn create_event(
    access_token: &str,
    calendar_id: &str,
    summary: &str,
    start_unix: i64,
    end_unix: i64,
    all_day: bool,
) -> Result<EventInfo, String> {
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events",
        path_encode(calendar_id)
    );
    let resp = agent()
        .post(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .send_json(build_event_body(summary, start_unix, end_unix, all_day))
        .map_err(|e| format!("events.insert request failed ({calendar_id}): {e}"))?;
    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad events.insert response: {e}"))?;
    parse_event(&json).ok_or_else(|| "events.insert: malformed response".to_string())
}

/// `PUT /calendars/{id}/events/{eventId}` — overwrite an existing event's
/// summary/time. Same primary-only restriction as [`create_event`].
pub fn update_event(
    access_token: &str,
    calendar_id: &str,
    event_id: &str,
    summary: &str,
    start_unix: i64,
    end_unix: i64,
    all_day: bool,
) -> Result<EventInfo, String> {
    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
        path_encode(calendar_id),
        path_encode(event_id)
    );
    let resp = agent()
        .put(&url)
        .set("Authorization", &format!("Bearer {access_token}"))
        .send_json(build_event_body(summary, start_unix, end_unix, all_day))
        .map_err(|e| format!("events.update request failed ({event_id}): {e}"))?;
    let json: Value = resp
        .into_json()
        .map_err(|e| format!("bad events.update response: {e}"))?;
    parse_event(&json).ok_or_else(|| "events.update: malformed response".to_string())
}

// --- pure helpers (unit-tested) ---

/// Build the `events.insert`/`events.update` request body. All-day events use
/// `{"date": "YYYY-MM-DD"}` per GCal's exclusive-end-date convention (caller
/// passes `end_unix` already one day past the last included day); timed
/// events use RFC3339 `dateTime`, matching what [`parse_time_point`] reads back.
fn build_event_body(summary: &str, start_unix: i64, end_unix: i64, all_day: bool) -> Value {
    let (start, end) = if all_day {
        (
            serde_json::json!({ "date": unix_to_date_only(start_unix) }),
            serde_json::json!({ "date": unix_to_date_only(end_unix) }),
        )
    } else {
        (
            serde_json::json!({ "dateTime": unix_to_rfc3339(start_unix) }),
            serde_json::json!({ "dateTime": unix_to_rfc3339(end_unix) }),
        )
    };
    serde_json::json!({ "summary": summary, "start": start, "end": end })
}

fn parse_calendar_list(v: &Value) -> Vec<CalendarInfo> {
    v.get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_calendar_item).collect())
        .unwrap_or_default()
}

fn parse_calendar_item(it: &Value) -> Option<CalendarInfo> {
    let gcal_id = it.get("id")?.as_str()?.to_string();
    let summary = it
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let bg_color = it
        .get("backgroundColor")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let is_primary = it.get("primary").and_then(Value::as_bool).unwrap_or(false);
    Some(CalendarInfo {
        gcal_id,
        summary,
        bg_color,
        is_primary,
    })
}

fn parse_events(v: &Value) -> Vec<EventInfo> {
    v.get("items")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(parse_event).collect())
        .unwrap_or_default()
}

fn parse_event(it: &Value) -> Option<EventInfo> {
    // Cancelled recurring-instance exceptions carry no start/end — skip them
    // rather than letting the `?` chain below fail the whole batch.
    if it.get("status").and_then(Value::as_str) == Some("cancelled") {
        return None;
    }
    let event_id = it.get("id")?.as_str()?.to_string();
    let summary = it
        .get("summary")
        .and_then(Value::as_str)
        .unwrap_or("(no title)")
        .to_string();
    let (start_unix, all_day) = parse_time_point(it.get("start")?)?;
    let (end_unix, _) = parse_time_point(it.get("end")?)?;
    let updated_unix = it
        .get("updated")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339)
        .unwrap_or(0);
    let etag = it
        .get("etag")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Some(EventInfo {
        event_id,
        summary,
        start_unix,
        end_unix,
        all_day,
        updated_unix,
        etag,
    })
}

/// GCal's `start`/`end` object is either `{"date": "YYYY-MM-DD"}` (all-day) or
/// `{"dateTime": RFC3339}` (timed). Returns `(unix_seconds, is_all_day)`.
fn parse_time_point(v: &Value) -> Option<(i64, bool)> {
    if let Some(dt) = v.get("dateTime").and_then(Value::as_str) {
        return Some((parse_rfc3339(dt)?, false));
    }
    if let Some(d) = v.get("date").and_then(Value::as_str) {
        return Some((parse_date_only(d)?, true));
    }
    None
}

/// Percent-encode a calendar id for use as a URL path segment (needed for
/// `@`/`.`-bearing ids like `xxx@group.calendar.google.com`).
fn path_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Days since the Unix epoch for a proleptic-Gregorian civil date (Howard
/// Hinnant's `days_from_civil`; see http://howardhinnant.github.io/date_algorithms.html).
/// Only exercised on modern (post-1970) dates here, so no negative-year care is taken.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let mp = if m > 2 { m - 3 } else { m + 9 }; // [0, 11]
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146097 + doe - 719468
}

/// Inverse of `days_from_civil`, used to format `timeMin`/`timeMax` query params.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn parse_date_only(s: &str) -> Option<i64> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    Some(days_from_civil(y, m, d) * 86400)
}

/// Parse an RFC3339 timestamp (`2026-07-15T09:30:00[.sss](Z|±HH:MM)`) to unix
/// seconds. GCal's `dateTime`/`updated` fields are always exactly this shape.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let (date_part, rest) = s.split_once('T')?;
    let days = parse_date_only(date_part)?;

    let (time_part, offset_secs) = if let Some(stripped) = rest.strip_suffix('Z') {
        (stripped, 0i64)
    } else {
        let idx = rest.rfind(['+', '-'])?;
        let (t, tz) = rest.split_at(idx);
        (t, parse_offset(tz)?)
    };

    let time_part = time_part.split('.').next().unwrap_or(time_part);
    let mut hms = time_part.splitn(3, ':');
    let h: i64 = hms.next()?.parse().ok()?;
    let mi: i64 = hms.next()?.parse().ok()?;
    let sec: i64 = hms.next().unwrap_or("0").parse().ok()?;

    Some(days + h * 3600 + mi * 60 + sec - offset_secs)
}

/// `±HH:MM` → signed offset in seconds (UTC = local − offset).
fn parse_offset(tz: &str) -> Option<i64> {
    if let Some(rest) = tz.strip_prefix('-') {
        return parse_offset_digits(rest).map(|s| -s);
    }
    parse_offset_digits(tz.strip_prefix('+')?)
}

fn parse_offset_digits(hm: &str) -> Option<i64> {
    let (h, m) = hm.split_once(':')?;
    let h: i64 = h.parse().ok()?;
    let m: i64 = m.parse().ok()?;
    Some(h * 3600 + m * 60)
}

/// Format a unix timestamp as a bare `YYYY-MM-DD` date (UTC day boundary),
/// for all-day event `start`/`end` bodies.
fn unix_to_date_only(unix: i64) -> String {
    let (y, m, d) = civil_from_days(unix.div_euclid(86400));
    format!("{y:04}-{m:02}-{d:02}")
}

/// Format a unix timestamp as UTC RFC3339 for `timeMin`/`timeMax` query params.
fn unix_to_rfc3339(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let secs_of_day = unix.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let mi = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn epoch_round_trips() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn known_reference_date() {
        // Canonical Hinnant test vector for the algorithm.
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(civil_from_days(11017), (2000, 3, 1));
    }

    #[test]
    fn days_from_civil_round_trips_over_a_wide_range() {
        for days in (-40000..40000).step_by(97) {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "y={y} m={m} d={d}");
        }
    }

    #[test]
    fn parse_rfc3339_utc() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T00:00:01Z"), Some(1));
    }

    #[test]
    fn parse_rfc3339_with_offset() {
        // 09:30 at UTC-4 is 13:30 UTC the same day.
        let with_offset = parse_rfc3339("2026-07-15T09:30:00-04:00").unwrap();
        let utc_equiv = parse_rfc3339("2026-07-15T13:30:00Z").unwrap();
        assert_eq!(with_offset, utc_equiv);
    }

    #[test]
    fn parse_rfc3339_with_fractional_seconds() {
        assert_eq!(
            parse_rfc3339("2026-07-15T09:30:00.123Z"),
            parse_rfc3339("2026-07-15T09:30:00Z")
        );
    }

    #[test]
    fn unix_to_rfc3339_round_trips_parse_rfc3339() {
        let unix = 1_784_000_000;
        let formatted = unix_to_rfc3339(unix);
        assert!(formatted.ends_with('Z'));
        assert_eq!(parse_rfc3339(&formatted), Some(unix));
    }

    #[test]
    fn parse_calendar_list_extracts_fields() {
        let v = json!({
            "items": [
                {"id": "primary", "summary": "Work", "backgroundColor": "#123456", "primary": true},
                {"id": "other@group.calendar.google.com", "summary": "Gym"}
            ]
        });
        let cals = parse_calendar_list(&v);
        assert_eq!(cals.len(), 2);
        assert_eq!(cals[0].gcal_id, "primary");
        assert!(cals[0].is_primary);
        assert_eq!(cals[1].summary, "Gym");
        assert!(!cals[1].is_primary);
    }

    #[test]
    fn parse_calendar_list_missing_items_yields_empty() {
        assert!(parse_calendar_list(&json!({})).is_empty());
    }

    #[test]
    fn parse_events_timed_and_all_day() {
        let v = json!({
            "items": [
                {
                    "id": "evt1",
                    "summary": "Standup",
                    "start": {"dateTime": "2026-07-15T09:00:00-04:00"},
                    "end": {"dateTime": "2026-07-15T09:15:00-04:00"},
                    "updated": "2026-07-01T00:00:00Z",
                    "etag": "\"abc\""
                },
                {
                    "id": "evt2",
                    "summary": "Vacation",
                    "start": {"date": "2026-07-20"},
                    "end": {"date": "2026-07-22"}
                }
            ]
        });
        let events = parse_events(&v);
        assert_eq!(events.len(), 2);
        assert!(!events[0].all_day);
        assert!(events[0].end_unix > events[0].start_unix);
        assert!(events[1].all_day);
        assert_eq!(events[1].summary, "Vacation");
    }

    #[test]
    fn parse_events_skips_cancelled_and_malformed() {
        let v = json!({
            "items": [
                {"id": "cancelled1", "status": "cancelled"},
                {"id": "no-start"},
            ]
        });
        assert!(parse_events(&v).is_empty());
    }

    #[test]
    fn parse_events_defaults_missing_summary() {
        let v = json!({
            "items": [
                {"id": "e1", "start": {"date": "2026-01-01"}, "end": {"date": "2026-01-02"}}
            ]
        });
        assert_eq!(parse_events(&v)[0].summary, "(no title)");
    }

    #[test]
    fn path_encode_escapes_at_keeps_dots() {
        assert_eq!(
            path_encode("foo@group.calendar.google.com"),
            "foo%40group.calendar.google.com"
        );
    }

    #[test]
    fn build_event_body_timed_uses_date_time() {
        let body = build_event_body("Standup", 1_784_000_000, 1_784_003_600, false);
        assert_eq!(body["summary"], "Standup");
        assert!(body["start"]["dateTime"].is_string());
        assert!(body["start"].get("date").is_none());
        assert!(body["end"]["dateTime"].is_string());
    }

    #[test]
    fn build_event_body_all_day_uses_date() {
        let body = build_event_body("Vacation", 1_784_000_000, 1_784_086_400, true);
        assert!(body["start"]["date"].is_string());
        assert!(body["start"].get("dateTime").is_none());
        assert!(body["end"]["date"].is_string());
    }

    #[test]
    fn unix_to_date_only_matches_civil_from_days() {
        assert_eq!(unix_to_date_only(0), "1970-01-01");
        assert_eq!(unix_to_date_only(86400), "1970-01-02");
    }
}
