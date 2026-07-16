//! §6.2/§6.7 `app_usage` refresh: aggregate 90 days of ActivityWatch window
//! events into per-app foreground minutes, cached in the app-owned `app_usage`
//! table so the Tools selector can usage-sort without a live AW round-trip.
//!
//! Mirrors nudge-svc's `aw_query.rs` conventions (prefix bucket match, tight
//! timeout, every failure collapses to "no data" instead of an error the UI has
//! to handle) but is deliberately app-local — the svc never reads `app_usage`,
//! so putting the aggregation here keeps the resident process free of the
//! (one-shot, heavier) 90-day range read. Refresh cadence is the same 24h cap
//! policy as `refresh_calendars` (§6.7), gated by the caller via `meta`.
//!
//! AW REST shapes used:
//!   GET /api/0/buckets/                                   -> { "<id>": {..} }
//!   GET /api/0/buckets/<id>/events?start=..&end=..        -> [ { "data": {"app": ..}, "duration": secs } ]

use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// AW's default REST bind address (same as nudge-svc `aw_query`).
const DEFAULT_BASE: &str = "http://localhost:5600";

/// Connect budget stays tight (AW-down must fail fast), but the read budget is
/// looser than the svc's 600ms — a 90-day event dump is a genuinely large body.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(600);
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Fetch and aggregate the last `days` days of window events into
/// `(app_name, minutes)` pairs. Empty on any AW failure — "no data", not error.
pub fn fetch_usage(days: i64, now: i64) -> Vec<(String, i64)> {
    fetch_usage_at(DEFAULT_BASE, days, now)
}

fn fetch_usage_at(base: &str, days: i64, now: i64) -> Vec<(String, i64)> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout_read(READ_TIMEOUT)
        .build();

    let buckets = match get_json(&agent, &format!("{base}/api/0/buckets/")) {
        Some(v) => v,
        None => return Vec::new(),
    };
    let bucket = match pick_bucket(&buckets, "aw-watcher-window") {
        Some(b) => b,
        None => return Vec::new(),
    };

    let start = rfc3339_utc(now - days * 86_400);
    let end = rfc3339_utc(now);
    let url = format!("{base}/api/0/buckets/{bucket}/events?start={start}&end={end}");
    match get_json(&agent, &url) {
        Some(events) => aggregate(&events),
        None => Vec::new(),
    }
}

/// Sum event `duration` seconds per `data.app`, returned as whole minutes,
/// most-used first. Pure — unit-tested against literal AW JSON.
fn aggregate(events: &Value) -> Vec<(String, i64)> {
    let mut secs: BTreeMap<String, f64> = BTreeMap::new();
    for e in events.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let app = match e.pointer("/data/app").and_then(Value::as_str) {
            Some(a) if !a.is_empty() => a,
            _ => continue,
        };
        let dur = e.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
        *secs.entry(app.to_string()).or_insert(0.0) += dur;
    }
    let mut out: Vec<(String, i64)> = secs
        .into_iter()
        .map(|(app, s)| (app, (s / 60.0) as i64))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// GET a URL and parse the body as JSON; `None` on any transport/parse error.
fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    agent.get(url).call().ok()?.into_json().ok()
}

/// First bucket id whose key starts with `prefix` (AW suffixes the hostname).
fn pick_bucket(buckets: &Value, prefix: &str) -> Option<String> {
    buckets
        .as_object()?
        .keys()
        .find(|k| k.starts_with(prefix))
        .cloned()
}

/// Unix seconds → the `YYYY-MM-DDTHH:MM:SSZ` form AW's `start`/`end` params
/// accept. Uses chrono (already a dependency) — no hand-rolled calendar here.
fn rfc3339_utc(unix: i64) -> String {
    use chrono::{TimeZone, Utc};
    match Utc.timestamp_opt(unix, 0).single() {
        Some(dt) => dt.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        None => "1970-01-01T00:00:00Z".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn aggregates_seconds_to_minutes_per_app() {
        let events = json!([
            {"data": {"app": "code.exe"}, "duration": 3600.0},
            {"data": {"app": "chrome.exe"}, "duration": 90.0},
            {"data": {"app": "code.exe"}, "duration": 660.0},
            {"data": {"app": ""}, "duration": 100.0},       // empty app skipped
            {"data": {}, "duration": 100.0},                  // no app skipped
            {"data": {"app": "slack.exe"}},                   // no duration → 0
        ]);
        assert_eq!(
            aggregate(&events),
            vec![
                ("code.exe".to_string(), 71),
                ("chrome.exe".to_string(), 1),
                ("slack.exe".to_string(), 0),
            ]
        );
    }

    #[test]
    fn non_array_or_empty_aggregates_empty() {
        assert!(aggregate(&json!([])).is_empty());
        assert!(aggregate(&json!({"not": "an array"})).is_empty());
    }

    #[test]
    fn rfc3339_utc_formats_aw_query_param() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_704_164_645), "2024-01-02T03:04:05Z");
    }
}
