//! ActivityWatch bridge (session 18). Reads the local AW server for the user's
//! current activity so a future check-in can be *activity-informed*: if the user
//! is clearly at the keyboard we can soften or skip the "still on it?" nag.
//!
//! The acquisition layer hands back a plain [`Activity`] snapshot;
//! [`Activity::presence`] maps it to the core [`Presence`] signal the state
//! machine branches on at a check-in edge (session 19).
//! Everything is best-effort and synchronous: AW is a localhost service, so a
//! round-trip is sub-millisecond when it is up, and every failure mode (AW down,
//! timeout, malformed JSON, missing bucket) collapses to an all-`Unknown`
//! snapshot rather than an error the svc has to handle. The tight [`TIMEOUT`]
//! guarantees a dead AW can never stall the single-threaded message loop.
//!
//! AW REST shapes this module depends on:
//!   GET /api/0/buckets/                      -> { "<bucket_id>": {..}, .. }
//!   GET /api/0/buckets/<id>/events?limit=1   -> [ { "data": {..}, .. } ]
//! afk event data: { "status": "not-afk" | "afk" }
//! window event data: { "app": "claude.exe", "title": "Claude" }
//! Bucket ids are hostname-suffixed (`aw-watcher-afk_HOST`) so we match on the
//! `aw-watcher-afk` / `aw-watcher-window` prefix, not an exact id.

use nudge_core::Presence;
use serde_json::Value;
use std::time::Duration;

/// AW's default REST bind address.
const DEFAULT_BASE: &str = "http://localhost:5600";

/// Fail-fast budget for every AW call. Localhost + AW-up is instant; AW-down
/// must not block the message loop, hence the tight connect/read cap.
const TIMEOUT: Duration = Duration::from_millis(600);

/// Whether the user is at the keyboard, per AW's afk watcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Afk {
    /// AW reports the user actively at the machine ("not-afk").
    Active,
    /// AW reports the user away ("afk").
    Away,
    /// AW unreachable, no afk bucket, or an unrecognized status.
    Unknown,
}

impl Default for Afk {
    fn default() -> Self {
        Afk::Unknown
    }
}

/// Snapshot of what the user is doing right now, as seen by ActivityWatch.
/// Fields degrade to `Unknown`/`None` when AW is down — "no signal", not error.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Activity {
    pub afk: Afk,
    /// Foreground executable (e.g. "claude.exe"), if the window watcher is up.
    pub app: Option<String>,
    /// Foreground window title, if the window watcher is up.
    pub title: Option<String>,
    /// Unix time the latest afk event ended (its `timestamp` + `duration`), used
    /// to reject a stale snapshot. `None` when there is no afk event or its
    /// timestamp could not be parsed — treated as "not fresh".
    pub afk_end: Option<i64>,
}

impl Activity {
    /// Map this snapshot to the core [`Presence`] signal, guarding against a
    /// stopped AW watcher: if the latest afk event ended more than
    /// `max_stale_secs` before `now`, the watcher likely died and its last
    /// "not-afk" can't be trusted, so report `Unknown` and let the check-in
    /// prompt show. A fresh `afk`/`not-afk` maps to `Away`/`Active`.
    pub fn presence(&self, now: i64, max_stale_secs: i64) -> Presence {
        let fresh = self.afk_end.is_some_and(|end| now - end <= max_stale_secs);
        match self.afk {
            _ if !fresh => Presence::Unknown,
            Afk::Active => Presence::Active,
            Afk::Away => Presence::Away,
            Afk::Unknown => Presence::Unknown,
        }
    }
}

/// Probe the default AW endpoint (`localhost:5600`).
pub fn probe() -> Activity {
    probe_at(DEFAULT_BASE)
}

/// Probe an explicit AW base URL. Never fails: any error collapses to an
/// all-`Unknown` snapshot.
pub fn probe_at(base: &str) -> Activity {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(TIMEOUT)
        .timeout_read(TIMEOUT)
        .build();

    let buckets = match get_json(&agent, &format!("{base}/api/0/buckets/")) {
        Some(v) => v,
        None => return Activity::default(),
    };

    let afk_event = pick_bucket(&buckets, "aw-watcher-afk")
        .and_then(|id| latest_event(&agent, base, &id));
    let afk = afk_event.as_ref().map(parse_afk).unwrap_or(Afk::Unknown);
    let afk_end = afk_event.as_ref().and_then(event_end);

    let (app, title) = pick_bucket(&buckets, "aw-watcher-window")
        .and_then(|id| latest_event(&agent, base, &id))
        .map(|e| parse_window(&e))
        .unwrap_or((None, None));

    Activity { afk, app, title, afk_end }
}

/// GET a URL and parse the body as JSON; `None` on any transport/parse error.
fn get_json(agent: &ureq::Agent, url: &str) -> Option<Value> {
    agent.get(url).call().ok()?.into_json().ok()
}

/// Fetch the single most recent event from a bucket.
fn latest_event(agent: &ureq::Agent, base: &str, bucket_id: &str) -> Option<Value> {
    let url = format!("{base}/api/0/buckets/{bucket_id}/events?limit=1");
    let arr = get_json(agent, &url)?;
    arr.get(0).cloned()
}

// --- pure helpers (unit-tested against literal AW JSON) ---

/// First bucket id whose key starts with `prefix` (AW suffixes the hostname).
fn pick_bucket(buckets: &Value, prefix: &str) -> Option<String> {
    buckets
        .as_object()?
        .keys()
        .find(|k| k.starts_with(prefix))
        .cloned()
}

/// Map an afk event's `data.status` to [`Afk`].
fn parse_afk(event: &Value) -> Afk {
    match event.pointer("/data/status").and_then(Value::as_str) {
        Some("not-afk") => Afk::Active,
        Some("afk") => Afk::Away,
        _ => Afk::Unknown,
    }
}

/// Pull `(app, title)` out of a window event's `data`.
fn parse_window(event: &Value) -> (Option<String>, Option<String>) {
    let field = |p: &str| event.pointer(p).and_then(Value::as_str).map(str::to_owned);
    (field("/data/app"), field("/data/title"))
}

/// Unix time an event ended: its RFC3339 `timestamp` + `duration` seconds.
/// `None` if the timestamp is missing/unparseable (duration defaults to 0).
fn event_end(event: &Value) -> Option<i64> {
    let start = parse_rfc3339(event.get("timestamp")?.as_str()?)?;
    let dur = event.get("duration").and_then(Value::as_f64).unwrap_or(0.0);
    Some(start + dur as i64)
}

/// Parse the RFC3339 timestamps AW emits (`2024-01-02T03:04:05.678000+00:00`,
/// also `...Z` and `±HH:MM`) to whole Unix seconds. Fractional seconds are
/// truncated. Deliberately hand-rolled — the crate carries no date library and
/// AW's format is fixed. `None` on any shape it doesn't recognize.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    // Minimum "YYYY-MM-DDTHH:MM:SS" is 19 bytes.
    if b.len() < 19 || b[4] != b'-' || b[7] != b'-' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: i64 = s.get(5..7)?.parse().ok()?;
    let day: i64 = s.get(8..10)?.parse().ok()?;
    let hour: i64 = s.get(11..13)?.parse().ok()?;
    let min: i64 = s.get(14..16)?.parse().ok()?;
    let sec: i64 = s.get(17..19)?.parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Strip optional ".fraction", then read the zone that follows.
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let end = frac.find(|c: char| !c.is_ascii_digit()).unwrap_or(frac.len());
        rest = &frac[end..];
    }
    let offset = parse_offset(rest)?;

    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + hour * 3600 + min * 60 + sec - offset)
}

/// Zone suffix → offset seconds east of UTC. `Z`/empty = 0; `±HH:MM`.
fn parse_offset(z: &str) -> Option<i64> {
    match z.as_bytes().first() {
        None | Some(b'Z') | Some(b'z') => Some(0),
        Some(&sign) if sign == b'+' || sign == b'-' => {
            let oh: i64 = z.get(1..3)?.parse().ok()?;
            let om: i64 = z.get(4..6)?.parse().ok()?;
            let mag = oh * 3600 + om * 60;
            Some(if sign == b'+' { mag } else { -mag })
        }
        _ => None,
    }
}

/// Days from the Unix epoch (1970-01-01) to `y-m-d`, proleptic Gregorian.
/// Howard Hinnant's `days_from_civil` — exact for all years, no lookup tables.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn picks_hostname_suffixed_bucket() {
        let buckets = json!({
            "aw-watcher-afk_HOST": {},
            "aw-watcher-window_HOST": {},
            "aw-watcher-web-chrome": {},
        });
        assert_eq!(
            pick_bucket(&buckets, "aw-watcher-afk").as_deref(),
            Some("aw-watcher-afk_HOST")
        );
        assert_eq!(
            pick_bucket(&buckets, "aw-watcher-window").as_deref(),
            Some("aw-watcher-window_HOST")
        );
        assert_eq!(pick_bucket(&buckets, "aw-watcher-missing"), None);
    }

    #[test]
    fn afk_status_maps_both_ways() {
        assert_eq!(parse_afk(&json!({"data": {"status": "not-afk"}})), Afk::Active);
        assert_eq!(parse_afk(&json!({"data": {"status": "afk"}})), Afk::Away);
        // Unknown/absent status never masquerades as Active.
        assert_eq!(parse_afk(&json!({"data": {"status": "???"}})), Afk::Unknown);
        assert_eq!(parse_afk(&json!({"data": {}})), Afk::Unknown);
    }

    #[test]
    fn window_extracts_app_and_title() {
        let e = json!({"data": {"app": "claude.exe", "title": "Claude"}});
        assert_eq!(
            parse_window(&e),
            (Some("claude.exe".into()), Some("Claude".into()))
        );
        // Titles with quotes/unicode survive (serde_json handles escaping).
        let weird = json!({"data": {"app": "x.exe", "title": "a \"b\" — 世界"}});
        assert_eq!(parse_window(&weird).1.as_deref(), Some("a \"b\" — 世界"));
        // Missing fields → None, not empty string.
        assert_eq!(parse_window(&json!({"data": {}})), (None, None));
    }

    #[test]
    fn default_activity_is_all_unknown() {
        let a = Activity::default();
        assert_eq!(a.afk, Afk::Unknown);
        assert_eq!(a, Activity { afk: Afk::Unknown, app: None, title: None, afk_end: None });
    }

    #[test]
    fn rfc3339_parses_aw_shapes() {
        // AW's canonical microsecond-UTC form.
        assert_eq!(parse_rfc3339("2024-01-02T03:04:05.678000+00:00"), Some(1_704_164_645));
        // Same instant with a Z suffix and no fraction.
        assert_eq!(parse_rfc3339("2024-01-02T03:04:05Z"), Some(1_704_164_645));
        // A non-UTC offset is applied (east of UTC subtracts to reach epoch).
        assert_eq!(parse_rfc3339("2024-01-02T03:04:05+01:00"), Some(1_704_164_645 - 3600));
        // The Unix epoch itself.
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        // Garbage / wrong shape → None, never a bogus time.
        assert_eq!(parse_rfc3339("not-a-timestamp"), None);
        assert_eq!(parse_rfc3339("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn event_end_adds_duration_to_timestamp() {
        let e = json!({"timestamp": "2024-01-02T03:04:05Z", "duration": 42.9});
        assert_eq!(event_end(&e), Some(1_704_164_645 + 42));
        // Missing duration → treat as zero-length.
        let e0 = json!({"timestamp": "2024-01-02T03:04:05Z"});
        assert_eq!(event_end(&e0), Some(1_704_164_645));
        // No timestamp → None.
        assert_eq!(event_end(&json!({"duration": 10.0})), None);
    }

    #[test]
    fn presence_respects_freshness_and_status() {
        let now = 1_000_000;
        let active = |end| Activity { afk: Afk::Active, afk_end: Some(end), ..Default::default() };
        // Fresh not-afk → Active.
        assert_eq!(active(now - 30).presence(now, 180), Presence::Active);
        // Stale not-afk (watcher stopped 10 min ago) → Unknown, so we still nag.
        assert_eq!(active(now - 600).presence(now, 180), Presence::Unknown);
        // Fresh afk → Away.
        let away = Activity { afk: Afk::Away, afk_end: Some(now - 5), ..Default::default() };
        assert_eq!(away.presence(now, 180), Presence::Away);
        // No afk_end (AW down) → Unknown regardless of status.
        assert_eq!(Activity { afk: Afk::Active, ..Default::default() }.presence(now, 180), Presence::Unknown);
    }
}
