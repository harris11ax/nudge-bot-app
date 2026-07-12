//! Pure escalation ladder for an unacknowledged task-start prompt.
//!
//! Given when a prompt first became visible (`shown_at`) and the current time,
//! answer two questions with zero tick logic: (1) what visibility LEVEL should
//! the prompt show right now, and (2) when is the next escalation edge the svc
//! must arm a timer for. No OS calls, no clock reads — time enters only as
//! `UnixTime` arguments (see [`crate::UnixTime`]).
//!
//! The ladder is deliberately monotone: level only rises while the prompt is
//! ignored. Acknowledging (Start), snoozing, or skipping is handled in
//! `state.rs` by dropping the prompt or moving `shown_at` forward; this module
//! never sees those events.

use crate::UnixTime;

/// Escalation timing, later sourced from rules.toml (`[escalation]`). Defaults
/// match the launch decision: gentle at 0, brighter at +5 min, loud at +10 min
/// and re-alerting every 10 min thereafter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ladder {
    /// Seconds after `shown_at` at which the prompt rises to [`Level::L1`].
    pub l1_after_secs: i64,
    /// Seconds after `shown_at` at which the prompt rises to [`Level::L2`].
    pub l2_after_secs: i64,
    /// Re-alert cadence (seconds) once at [`Level::L2`]. `0` disables re-alerts.
    pub l2_repeat_secs: i64,
}

impl Default for Ladder {
    fn default() -> Self {
        Ladder {
            l1_after_secs: 5 * 60,
            l2_after_secs: 10 * 60,
            l2_repeat_secs: 10 * 60,
        }
    }
}

/// Visibility tier of an active prompt. Higher = more intrusive. Semantics for
/// the svc: L0 gentle anchor text, L1 brighter/persistent, L2 loud + sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    L0,
    L1,
    L2,
}

/// Ladder evaluation at a point in time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// Level to display right now.
    pub level: Level,
    /// Absolute time of the next escalation edge, or `None` once the ladder is
    /// fully climbed and re-alerts are disabled (nothing left to arm).
    pub next_step_at: Option<UnixTime>,
    /// True exactly on the tick a fresh L2 re-alert fires (svc plays the sound
    /// again). Never set for the initial climb into L1/L2, which the level
    /// change already signals.
    pub realert: bool,
}

/// Evaluate the ladder for a prompt shown at `shown_at`, as of `now`.
///
/// `now` before `shown_at` is treated as `shown_at` (clamped): a not-yet-shown
/// prompt is L0 with its first edge ahead.
pub fn evaluate(ladder: &Ladder, shown_at: UnixTime, now: UnixTime) -> Step {
    let elapsed = (now - shown_at).max(0);

    if elapsed < ladder.l1_after_secs {
        return Step {
            level: Level::L0,
            next_step_at: Some(shown_at + ladder.l1_after_secs),
            realert: false,
        };
    }
    if elapsed < ladder.l2_after_secs {
        return Step {
            level: Level::L1,
            next_step_at: Some(shown_at + ladder.l2_after_secs),
            realert: false,
        };
    }

    // At or past L2.
    if ladder.l2_repeat_secs <= 0 {
        return Step {
            level: Level::L2,
            next_step_at: None,
            realert: false,
        };
    }
    let since_l2 = elapsed - ladder.l2_after_secs;
    let elapsed_periods = since_l2 / ladder.l2_repeat_secs;
    let realert = since_l2 % ladder.l2_repeat_secs == 0 && since_l2 > 0;
    let next_at = shown_at + ladder.l2_after_secs + (elapsed_periods + 1) * ladder.l2_repeat_secs;
    Step {
        level: Level::L2,
        next_step_at: Some(next_at),
        realert,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const L: Ladder = Ladder {
        l1_after_secs: 300,
        l2_after_secs: 600,
        l2_repeat_secs: 600,
    };

    #[test]
    fn fresh_prompt_is_l0_next_edge_at_l1() {
        let s = evaluate(&L, 1000, 1000);
        assert_eq!(s.level, Level::L0);
        assert_eq!(s.next_step_at, Some(1300));
        assert!(!s.realert);
    }

    #[test]
    fn just_before_l1_still_l0() {
        let s = evaluate(&L, 1000, 1299);
        assert_eq!(s.level, Level::L0);
        assert_eq!(s.next_step_at, Some(1300));
    }

    #[test]
    fn at_l1_boundary_rises_and_targets_l2() {
        let s = evaluate(&L, 1000, 1300);
        assert_eq!(s.level, Level::L1);
        assert_eq!(s.next_step_at, Some(1600));
        assert!(!s.realert);
    }

    #[test]
    fn at_l2_boundary_first_period_no_realert() {
        let s = evaluate(&L, 1000, 1600);
        assert_eq!(s.level, Level::L2);
        assert_eq!(s.next_step_at, Some(2200)); // 1600 + one repeat
        assert!(!s.realert);
    }

    #[test]
    fn on_a_repeat_tick_realert_fires() {
        let s = evaluate(&L, 1000, 2200); // l2 + exactly one repeat
        assert_eq!(s.level, Level::L2);
        assert!(s.realert);
        assert_eq!(s.next_step_at, Some(2800));
    }

    #[test]
    fn between_repeats_no_realert_next_edge_correct() {
        let s = evaluate(&L, 1000, 2500); // 900s past l2, mid second period
        assert_eq!(s.level, Level::L2);
        assert!(!s.realert);
        assert_eq!(s.next_step_at, Some(2800));
    }

    #[test]
    fn repeat_disabled_has_no_next_edge() {
        let ladder = Ladder { l2_repeat_secs: 0, ..L };
        let s = evaluate(&ladder, 1000, 5000);
        assert_eq!(s.level, Level::L2);
        assert_eq!(s.next_step_at, None);
        assert!(!s.realert);
    }

    #[test]
    fn now_before_shown_clamps_to_l0() {
        let s = evaluate(&L, 1000, 500);
        assert_eq!(s.level, Level::L0);
        assert_eq!(s.next_step_at, Some(1300));
    }

    #[test]
    fn default_ladder_matches_launch_decision() {
        let d = Ladder::default();
        assert_eq!(d.l1_after_secs, 300);
        assert_eq!(d.l2_after_secs, 600);
        assert_eq!(d.l2_repeat_secs, 600);
    }
}
