//! Parse/validate rules.toml. Nudge/escalation subset only; escalation-ladder
//! settings are added here later without touching state.rs or schedule.rs.

use crate::escalate::Ladder;
use crate::Mode;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct Rules {
    pub anchor: Anchor,
    #[serde(default, rename = "nudge")]
    pub nudges: Vec<NudgeWindow>,
    pub hotkey: Option<Hotkey>,
    #[serde(default)]
    pub escalation: Escalation,
    #[serde(default)]
    pub classify: Classify,
}

/// On/off-task classification config (global; `[classify]` in rules.toml). The
/// `productive_apps` list names foreground executables that mean "already
/// working" — a match at the edge selects [`Mode::OnTask`]. Optional block; an
/// empty list makes every prompt [`Mode::OffTask`] (the launch behaviour before
/// UI-P0). Per-task app lists come later (UI-PLAN "productive-app list
/// global-only for P0").
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct Classify {
    /// Foreground executables that count as on-task, e.g. `["code.exe",
    /// "idea64.exe"]`. Matched case-insensitively against AW's window app.
    pub productive_apps: Vec<String>,
}

impl Classify {
    /// Classify the current foreground executable. A case-insensitive exact
    /// match against `productive_apps` → [`Mode::OnTask`]; anything else,
    /// including no foreground signal (`None`, i.e. AW down), → [`Mode::OffTask`].
    pub fn mode(&self, app: Option<&str>) -> Mode {
        match app {
            Some(a) if self.productive_apps.iter().any(|p| p.eq_ignore_ascii_case(a)) => {
                Mode::OnTask
            }
            _ => Mode::OffTask,
        }
    }
}

/// Escalation-ladder, snooze, and check-in timing (global; `[escalation]` in
/// rules.toml). Every field defaults, so the whole block is optional. Feeds the
/// runtime edges in [`crate::state`] via [`crate::schedule::context`].
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Escalation {
    /// Seconds after a prompt first shows before it rises to L1.
    pub l1_after_secs: i64,
    /// Seconds after a prompt first shows before it rises to L2.
    pub l2_after_secs: i64,
    /// Re-alert cadence (seconds) once at L2; 0 disables re-alerts.
    pub l2_repeat_secs: i64,
    /// Snooze duration in seconds.
    pub snooze_secs: i64,
    /// Delay after Start before a "still on it?" check-in; 0 disables check-ins.
    pub checkin_after_secs: i64,
    /// STARTED-mode AW sampling cadence in seconds (§6.1); 0 disables sampling.
    /// Off by default, mirroring `checkin_after_secs` — an existing rules.toml
    /// keeps its pre-Phase-3 behaviour until the user opts in.
    pub sample_secs: i64,
    /// How long a *continuous* off-task run must last before the drift check-in
    /// fires (§6.5). Measured from the first off-task sample, so with the default
    /// cadence it takes two consecutive off-task samples to cross.
    pub off_task_secs: i64,
    /// How long "Take a break" from the §6.5 check-in silences everything.
    pub break_secs: i64,
    /// How long a tray Pause silences everything (§6.6).
    pub pause_secs: i64,
}

impl Default for Escalation {
    fn default() -> Self {
        Escalation {
            l1_after_secs: 5 * 60,
            l2_after_secs: 10 * 60,
            l2_repeat_secs: 10 * 60,
            snooze_secs: 10 * 60,
            checkin_after_secs: 0,
            sample_secs: 0,
            off_task_secs: 5 * 60,
            break_secs: 10 * 60,
            pause_secs: 30 * 60,
        }
    }
}

impl Escalation {
    /// The pure escalation ladder these settings describe.
    pub fn ladder(&self) -> Ladder {
        Ladder {
            l1_after_secs: self.l1_after_secs,
            l2_after_secs: self.l2_after_secs,
            l2_repeat_secs: self.l2_repeat_secs,
        }
    }

    /// Check-in delay as the state machine wants it: `None` when disabled.
    pub fn checkin(&self) -> Option<i64> {
        (self.checkin_after_secs > 0).then_some(self.checkin_after_secs)
    }

    /// Sampling cadence as the state machine wants it: `None` when disabled, in
    /// which case `Started` never carries a `sample_at` and no sample edge can be
    /// armed at all (§6.1 zero-polling guarantee).
    pub fn sample(&self) -> Option<i64> {
        (self.sample_secs > 0).then_some(self.sample_secs)
    }
}

#[derive(Debug, Deserialize)]
pub struct Anchor {
    pub default_text: String,
    #[serde(default = "default_position")]
    pub position: String,
    #[serde(default = "default_height")]
    pub height_px: u32,
}

fn default_position() -> String {
    "top".into()
}
fn default_height() -> u32 {
    28
}

#[derive(Debug, Deserialize)]
pub struct NudgeWindow {
    pub name: String,
    /// Lowercase three-letter day names: mon..sun.
    pub days: Vec<String>,
    /// "HH:MM" local time.
    pub start: String,
    pub end: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub struct Hotkey {
    pub modifiers: Vec<String>,
    pub key: String,
}

#[derive(Debug)]
pub enum RulesError {
    Parse(String),
    Invalid(String),
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Minutes since local midnight from "HH:MM".
pub fn parse_hhmm(s: &str) -> Result<u32, RulesError> {
    let (h, m) = s
        .split_once(':')
        .ok_or_else(|| RulesError::Invalid(format!("bad time '{s}'")))?;
    let h: u32 = h.parse().map_err(|_| RulesError::Invalid(format!("bad hour '{s}'")))?;
    let m: u32 = m.parse().map_err(|_| RulesError::Invalid(format!("bad minute '{s}'")))?;
    if h > 23 || m > 59 {
        return Err(RulesError::Invalid(format!("out-of-range time '{s}'")));
    }
    Ok(h * 60 + m)
}

pub fn parse(toml_src: &str) -> Result<Rules, RulesError> {
    let rules: Rules = toml::from_str(toml_src).map_err(|e| RulesError::Parse(e.to_string()))?;
    for n in &rules.nudges {
        for d in &n.days {
            if !DAYS.contains(&d.as_str()) {
                return Err(RulesError::Invalid(format!("{}: bad day '{d}'", n.name)));
            }
        }
        let (s, e) = (parse_hhmm(&n.start)?, parse_hhmm(&n.end)?);
        if s >= e {
            return Err(RulesError::Invalid(format!("{}: start >= end", n.name)));
        }
    }
    let esc = &rules.escalation;
    if esc.l1_after_secs < 0
        || esc.l2_after_secs < 0
        || esc.snooze_secs < 0
        || esc.sample_secs < 0
        || esc.off_task_secs < 0
        || esc.break_secs < 0
        || esc.pause_secs < 0
    {
        return Err(RulesError::Invalid("escalation: negative duration".into()));
    }
    if esc.l1_after_secs > esc.l2_after_secs {
        return Err(RulesError::Invalid(
            "escalation: l1_after_secs > l2_after_secs".into(),
        ));
    }
    Ok(rules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mode;

    const OK: &str = r#"
[anchor]
default_text = "hi"
[[nudge]]
name = "a"
days = ["mon"]
start = "08:30"
end = "12:00"
text = "t"
"#;

    #[test]
    fn parses_valid() {
        let r = parse(OK).unwrap();
        assert_eq!(r.nudges.len(), 1);
        assert_eq!(r.anchor.height_px, 28);
    }

    #[test]
    fn rejects_bad_day_and_inverted_window() {
        assert!(parse(&OK.replace("mon", "xyz")).is_err());
        assert!(parse(&OK.replace("12:00", "08:00")).is_err());
    }

    #[test]
    fn hhmm() {
        assert_eq!(parse_hhmm("08:30").unwrap(), 510);
        assert!(parse_hhmm("24:00").is_err());
    }

    #[test]
    fn escalation_defaults_when_absent() {
        let r = parse(OK).unwrap();
        let l = r.escalation.ladder();
        assert_eq!(l.l1_after_secs, 300);
        assert_eq!(l.l2_after_secs, 600);
        assert_eq!(r.escalation.snooze_secs, 600);
        assert_eq!(r.escalation.checkin(), None); // check-ins off by default
        assert_eq!(r.escalation.sample(), None); // sampling off by default
        assert_eq!(r.escalation.off_task_secs, 300);
    }

    #[test]
    fn escalation_block_enables_sampling() {
        let src = format!("{OK}\n[escalation]\nsample_secs = 300\noff_task_secs = 600\n");
        let r = parse(&src).unwrap();
        assert_eq!(r.escalation.sample(), Some(300));
        assert_eq!(r.escalation.off_task_secs, 600);
        // A negative cadence is rejected like every other duration.
        assert!(parse(&format!("{OK}\n[escalation]\nsample_secs = -1\n")).is_err());
    }

    #[test]
    fn escalation_block_overrides_and_enables_checkin() {
        let src = format!("{OK}\n[escalation]\nsnooze_secs = 300\ncheckin_after_secs = 1800\n");
        let r = parse(&src).unwrap();
        assert_eq!(r.escalation.snooze_secs, 300);
        assert_eq!(r.escalation.checkin(), Some(1800));
        // Unspecified ladder fields keep their defaults.
        assert_eq!(r.escalation.ladder().l1_after_secs, 300);
    }

    #[test]
    fn rejects_inverted_ladder() {
        let src = format!("{OK}\n[escalation]\nl1_after_secs = 600\nl2_after_secs = 300\n");
        assert!(parse(&src).is_err());
    }

    #[test]
    fn classify_defaults_to_off_task_when_absent() {
        let r = parse(OK).unwrap();
        assert!(r.classify.productive_apps.is_empty());
        // Empty list: every foreground app, and no app at all, is off-task.
        assert_eq!(r.classify.mode(Some("code.exe")), Mode::OffTask);
        assert_eq!(r.classify.mode(None), Mode::OffTask);
    }

    #[test]
    fn classify_matches_productive_app_case_insensitively() {
        let src = format!("{OK}\n[classify]\nproductive_apps = [\"code.exe\", \"idea64.exe\"]\n");
        let c = parse(&src).unwrap().classify;
        assert_eq!(c.mode(Some("Code.exe")), Mode::OnTask); // case-insensitive
        assert_eq!(c.mode(Some("idea64.exe")), Mode::OnTask);
        assert_eq!(c.mode(Some("chrome.exe")), Mode::OffTask); // not listed
        assert_eq!(c.mode(None), Mode::OffTask); // AW down → off-task
    }
}
