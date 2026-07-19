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
//! §7.4(b) per-website resolution: browser exes (`brave.exe`, `chrome.exe`,
//! `msedge.exe`, `firefox.exe`) are attributed to the actual site rather than the
//! browser. Site time is sourced from the AW web-watcher buckets
//! (`aw-watcher-web-<browser>`, `data.url`/`data.title`) when the extension is
//! present, and browser window-events fall back to the window title's host.
//! Resolved browser time surfaces as `"<exe> → <host>"` entries (e.g.
//! `brave.exe → github.com`); unresolved browser time stays under the bare exe.
//! Budget-safe: the web buckets are read at the same 90-day cache-refresh cadence,
//! no new sampling.
//!
//! AW REST shapes used:
//!   GET /api/0/buckets/                                   -> { "<id>": {..} }
//!   GET /api/0/buckets/<id>/events?start=..&end=..        -> [ { "data": {"app": ..}, "duration": secs } ]
//!   web-watcher event data: { "url": "https://github.com/x", "title": ".." }

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Browser executables whose foreground time is resolved to the visited site.
const BROWSER_EXES: &[&str] = &["brave.exe", "chrome.exe", "msedge.exe", "firefox.exe"];

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
    let events_url = |id: &str| format!("{base}/api/0/buckets/{id}/events?start={start}&end={end}");
    let window = match get_json(&agent, &events_url(&bucket)) {
        Some(events) => events,
        None => return Vec::new(),
    };

    // Every web-watcher bucket (one per browser) that AW exposes, mapped to the
    // browser exe it belongs to. Absent when the extension isn't installed —
    // browsers then fall back to window-title host resolution.
    let web: Vec<(String, Value)> = web_buckets(&buckets)
        .into_iter()
        .filter_map(|(exe, id)| get_json(&agent, &events_url(&id)).map(|ev| (exe, ev)))
        .collect();

    aggregate(&window, &web)
}

/// Sum foreground `duration` seconds into per-tool minutes, most-used first.
/// Non-browser apps key on `data.app`; browser time keys on `"<exe> → <host>"`
/// where the host comes from the web-watcher `web` events (preferred) or the
/// window title (fallback). Pure — unit-tested against literal AW JSON.
fn aggregate(window: &Value, web: &[(String, Value)]) -> Vec<(String, i64)> {
    let mut secs: BTreeMap<String, f64> = BTreeMap::new();

    // Web-watcher first: authoritative per-site browser time. Also records which
    // browsers have web data so their window events aren't double-counted below.
    let mut browsers_with_web: BTreeSet<String> = BTreeSet::new();
    for (exe, events) in web {
        for e in events.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
            let host = e
                .pointer("/data/url")
                .and_then(Value::as_str)
                .and_then(host_from_url)
                .or_else(|| {
                    e.pointer("/data/title")
                        .and_then(Value::as_str)
                        .and_then(host_from_title)
                });
            let host = match host {
                Some(h) => h,
                None => continue,
            };
            browsers_with_web.insert(exe.clone());
            let dur = e.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
            *secs.entry(format!("{exe} → {host}")).or_insert(0.0) += dur;
        }
    }

    for e in window.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let app = match e.pointer("/data/app").and_then(Value::as_str) {
            Some(a) if !a.is_empty() => a,
            _ => continue,
        };
        let dur = e.get("duration").and_then(Value::as_f64).unwrap_or(0.0);

        let key = if is_browser(app) {
            // A browser with web-watcher data is already fully accounted for by
            // the loop above — skip its window events to avoid double-counting.
            if browsers_with_web.contains(&app.to_ascii_lowercase())
                || browsers_with_web.contains(app)
            {
                continue;
            }
            // No web bucket: try the window title's host, else leave under the exe.
            match e
                .pointer("/data/title")
                .and_then(Value::as_str)
                .and_then(host_from_title)
            {
                Some(host) => format!("{app} → {host}"),
                None => app.to_string(),
            }
        } else {
            app.to_string()
        };
        *secs.entry(key).or_insert(0.0) += dur;
    }

    let mut out: Vec<(String, i64)> = secs
        .into_iter()
        .map(|(app, s)| (app, (s / 60.0) as i64))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Is `app` one of the known browser executables (case-insensitive)?
fn is_browser(app: &str) -> bool {
    BROWSER_EXES.iter().any(|b| b.eq_ignore_ascii_case(app))
}

/// Every `aw-watcher-web*` bucket paired with the browser exe it belongs to.
/// The bucket id carries the browser name (`aw-watcher-web-chrome`); unknown
/// clients are dropped since we can't name their exe.
fn web_buckets(buckets: &Value) -> Vec<(String, String)> {
    let obj = match buckets.as_object() {
        Some(o) => o,
        None => return Vec::new(),
    };
    obj.keys()
        .filter(|k| k.starts_with("aw-watcher-web"))
        .filter_map(|k| browser_exe_for(k).map(|exe| (exe.to_string(), k.clone())))
        .collect()
}

/// Map a web-watcher bucket id to its browser exe by the client token it carries.
fn browser_exe_for(bucket_id: &str) -> Option<&'static str> {
    let id = bucket_id.to_ascii_lowercase();
    if id.contains("brave") {
        Some("brave.exe")
    } else if id.contains("edge") {
        Some("msedge.exe")
    } else if id.contains("firefox") {
        Some("firefox.exe")
    } else if id.contains("chrome") {
        Some("chrome.exe")
    } else {
        None
    }
}

/// Host of a URL: scheme/userinfo/port/path stripped, lowercased, `www.` dropped.
/// `None` if it doesn't look like a host (must contain a dot).
fn host_from_url(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host); // strip userinfo
    let host = host.split(':').next().unwrap_or(host); // strip port
    let host = host.trim().to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host);
    if host.is_empty() || !host.contains('.') {
        return None;
    }
    Some(host.to_string())
}

/// Best-effort host from a window/tab title: the last dotted, host-legal token.
/// Deliberately conservative — a title with no host-like token yields `None`.
fn host_from_title(title: &str) -> Option<String> {
    title
        .split_whitespace()
        .filter_map(|t| {
            let t = t.trim_matches(|c: char| !(c.is_ascii_alphanumeric()));
            let host_legal = t
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
            if t.contains('.') && host_legal {
                Some(t.to_ascii_lowercase())
            } else {
                None
            }
        })
        .filter_map(|h| host_from_url(&h))
        .last()
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
        // No web buckets: browser with no title-host stays under the bare exe.
        assert_eq!(
            aggregate(&events, &[]),
            vec![
                ("code.exe".to_string(), 71),
                ("chrome.exe".to_string(), 1),
                ("slack.exe".to_string(), 0),
            ]
        );
    }

    #[test]
    fn non_array_or_empty_aggregates_empty() {
        assert!(aggregate(&json!([]), &[]).is_empty());
        assert!(aggregate(&json!({"not": "an array"}), &[]).is_empty());
    }

    #[test]
    fn web_watcher_resolves_browser_time_per_site() {
        // Window shows 1h of brave + 10m of code; the brave web bucket splits
        // that browser time across two sites and supersedes the window rollup.
        let window = json!([
            {"data": {"app": "brave.exe", "title": "whatever"}, "duration": 3600.0},
            {"data": {"app": "code.exe"}, "duration": 600.0},
        ]);
        let web = vec![(
            "brave.exe".to_string(),
            json!([
                {"data": {"url": "https://github.com/foo/bar"}, "duration": 2400.0},
                {"data": {"url": "https://www.github.com/baz"}, "duration": 600.0}, // www. folded
                {"data": {"url": "https://mail.google.com/"}, "duration": 600.0},
                {"data": {"title": "no url here"}},                                 // unresolved skipped
            ]),
        )];
        assert_eq!(
            aggregate(&window, &web),
            vec![
                ("brave.exe → github.com".to_string(), 50),
                // tie at 10m broken by name ascending: "brave…" before "code.exe"
                ("brave.exe → mail.google.com".to_string(), 10),
                ("code.exe".to_string(), 10),
            ]
        );
    }

    #[test]
    fn browser_without_web_bucket_falls_back_to_title_host() {
        // No web bucket for chrome → resolve host from the window title; a
        // title with no host-like token stays under the bare exe.
        let window = json!([
            {"data": {"app": "chrome.exe", "title": "My Issue · github.com"}, "duration": 1200.0},
            {"data": {"app": "chrome.exe", "title": "New Tab"}, "duration": 600.0},
        ]);
        assert_eq!(
            aggregate(&window, &[]),
            vec![
                ("chrome.exe → github.com".to_string(), 20),
                ("chrome.exe".to_string(), 10),
            ]
        );
    }

    #[test]
    fn web_buckets_map_to_browser_exes() {
        let buckets = json!({
            "aw-watcher-window_HOST": {},
            "aw-watcher-web-brave": {},
            "aw-watcher-web-chrome": {},
            "aw-watcher-web-firefox": {},
            "aw-watcher-web-unknownclient": {},   // dropped: no exe
        });
        let mut wb = web_buckets(&buckets);
        wb.sort();
        assert_eq!(
            wb,
            vec![
                ("brave.exe".to_string(), "aw-watcher-web-brave".to_string()),
                ("chrome.exe".to_string(), "aw-watcher-web-chrome".to_string()),
                ("firefox.exe".to_string(), "aw-watcher-web-firefox".to_string()),
            ]
        );
    }

    #[test]
    fn host_parsing_normalizes_and_rejects_non_hosts() {
        assert_eq!(host_from_url("https://user@GitHub.com:443/x?q=1"), Some("github.com".to_string()));
        assert_eq!(host_from_url("http://www.example.com/"), Some("example.com".to_string()));
        assert_eq!(host_from_url("about:blank"), None); // no dot
        assert_eq!(host_from_title("Issue #3 — mail.google.com"), Some("mail.google.com".to_string()));
        assert_eq!(host_from_title("just a plain title"), None);
    }

    #[test]
    fn rfc3339_utc_formats_aw_query_param() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_704_164_645), "2024-01-02T03:04:05Z");
    }
}
