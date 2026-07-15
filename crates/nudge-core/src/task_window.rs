//! Pure task-list selection/sort/window/style — the sole owner of the §6.8
//! dynamic-deadline horizon and §6.9 row styling (UI-PLAN §6.9 "modularity is
//! load-bearing"). Both the svc §6.5 popup and the app `TaskListPanel` consume
//! [`display_list`]'s `Vec<Row>` verbatim: zero sorting/filtering/styling lives
//! in any render layer. OS-free and clock-free like the rest of nudge-core —
//! time enters only as `UnixTime`; `logged`/`estimate` enter as data.

use crate::tasks::Task;
use crate::UnixTime;
use std::collections::HashMap;

/// Per-task progress fed in by the caller (svc/app). Kept out of [`Task`] so this
/// module needs no schema change (`estimate_minutes`/`logged_minutes` land in
/// Phase 2); `estimate: None` = "no estimate" → remaining 0 → 48 h horizon.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub logged: u32,
    pub estimate: Option<u32>,
}

/// Task-id → [`Progress`]. Tasks absent from the map default to `Progress::default`
/// (logged 0, no estimate).
pub type LoggedMap = HashMap<i64, Progress>;

/// Style/window settings (from rules.toml Style tab, §6.9). `bands` maps a
/// `logged/estimate` ratio threshold to the class assigned at or above it; the
/// greatest threshold `<=` the ratio wins (thresholds expected ascending).
#[derive(Debug, Clone)]
pub struct WindowCfg {
    pub bands: Vec<(f32, StyleClass)>,
    /// Not-started rows flip to [`StyleClass::NotStartedUrgent`] when
    /// `deadline - now <` this. Default 24 h.
    pub not_started_red_before_secs: i64,
    /// Hard cap on rows returned (§6.5). Default 12.
    pub max_rows: usize,
}

impl Default for WindowCfg {
    fn default() -> Self {
        WindowCfg {
            bands: vec![
                (0.0, StyleClass::Band(0)),
                (0.5, StyleClass::Band(1)),
                (0.9, StyleClass::Band(2)),
            ],
            not_started_red_before_secs: 24 * 3600,
            max_rows: 12,
        }
    }
}

/// One display row, fully computed and ready to paint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub task_id: i64,
    pub title: String,
    pub deadline: Option<UnixTime>,
    pub logged: u32,
    pub estimate: Option<u32>,
    pub style: StyleClass,
}

/// Per-row paint class (§6.9). `Band(0)` is the lowest completion band.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleClass {
    /// Not started (logged 0), white bg / black outline.
    NotStarted,
    /// Not started and `< not_started_red_before_secs` to deadline → red outline.
    NotStartedUrgent,
    /// Started: completion band from `logged/estimate`.
    Band(u8),
}

/// §6.8 horizon (seconds): admit a task whose deadline is within this of `now`,
/// scaled by remaining work. Boundaries are inclusive at the lower edge
/// (180 min = 3 h → 72 h; 360 = 6 h → 96 h; 720 = 12 h → 168 h).
fn horizon_secs(remaining_min: u32) -> i64 {
    if remaining_min >= 12 * 60 {
        168 * 3600
    } else if remaining_min >= 6 * 60 {
        96 * 3600
    } else if remaining_min >= 3 * 60 {
        72 * 3600
    } else {
        48 * 3600
    }
}

/// Remaining work = `estimate - logged` (saturating); no estimate → 0 (§6.8).
fn remaining_min(p: &Progress) -> u32 {
    p.estimate.map(|e| e.saturating_sub(p.logged)).unwrap_or(0)
}

/// Resolve a started row's completion band from `cfg.bands`: greatest threshold
/// `<=` the `logged/estimate` ratio. No/zero estimate → ratio 0.
fn band(logged: u32, estimate: Option<u32>, cfg: &WindowCfg) -> StyleClass {
    let ratio = match estimate {
        Some(e) if e > 0 => logged as f32 / e as f32,
        _ => 0.0,
    };
    let mut chosen = StyleClass::Band(0);
    for (thresh, class) in &cfg.bands {
        if ratio >= *thresh {
            chosen = class.clone();
        }
    }
    chosen
}

/// §6.9 row style. Logged 0 → not-started (urgent if close to deadline);
/// otherwise a completion band.
fn style_for(logged: u32, estimate: Option<u32>, deadline: Option<UnixTime>, now: UnixTime, cfg: &WindowCfg) -> StyleClass {
    if logged == 0 {
        if let Some(dl) = deadline {
            if dl - now < cfg.not_started_red_before_secs {
                return StyleClass::NotStartedUrgent;
            }
        }
        return StyleClass::NotStarted;
    }
    band(logged, estimate, cfg)
}

/// The one entry point (§6.8/§6.9). Admits tasks whose deadline falls inside the
/// dynamic horizon for their remaining work, sorts soonest-deadline-first
/// regardless of the admitting horizon, caps to `cfg.max_rows`, and assigns each
/// row its style. Deadline-less tasks are never admitted (the window keys on the
/// deadline). Pure: no OS, no clock, no I/O.
pub fn display_list(tasks: &[Task], logged: &LoggedMap, now: UnixTime, cfg: &WindowCfg) -> Vec<Row> {
    let mut rows: Vec<Row> = tasks
        .iter()
        .filter_map(|t| {
            let id = t.id?;
            let deadline = t.deadline?;
            let p = logged.get(&id).copied().unwrap_or_default();
            if deadline - now > horizon_secs(remaining_min(&p)) {
                return None;
            }
            Some(Row {
                task_id: id,
                title: t.title.clone(),
                deadline: Some(deadline),
                logged: p.logged,
                estimate: p.estimate,
                style: style_for(p.logged, p.estimate, Some(deadline), now, cfg),
            })
        })
        .collect();
    // Sooner deadlines first, regardless of admitting horizon (§6.8).
    rows.sort_by_key(|r| r.deadline.unwrap_or(UnixTime::MAX));
    rows.truncate(cfg.max_rows);
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tasks::{Recur, TriggerSource};

    const H: i64 = 3600;

    fn task(id: i64, deadline: Option<UnixTime>) -> Task {
        Task {
            id: Some(id),
            title: format!("t{id}"),
            desc: String::new(),
            deadline,
            task_type: String::new(),
            minutes: None,
            recur: Recur::Once,
            mode_override: None,
            trigger_source: TriggerSource::Manual,
            gcal_event_id: None,
            estimate_minutes: None,
            logged_minutes: 0,
        }
    }

    fn prog(logged: u32, estimate: Option<u32>) -> Progress {
        Progress { logged, estimate }
    }

    // §6.8 horizon boundaries via remaining-work: 179 vs 180 min, 6 h, 12 h.
    #[test]
    fn horizon_boundaries() {
        assert_eq!(horizon_secs(179), 48 * H);
        assert_eq!(horizon_secs(180), 72 * H);
        assert_eq!(horizon_secs(359), 72 * H);
        assert_eq!(horizon_secs(360), 96 * H);
        assert_eq!(horizon_secs(719), 96 * H);
        assert_eq!(horizon_secs(720), 168 * H);
    }

    // Remaining just under 3 h → 48 h window: a task due in 60 h is excluded;
    // remaining ≥ 3 h widens to 72 h and admits it.
    #[test]
    fn admit_scales_with_remaining() {
        let now = 1_000_000;
        let t = task(1, Some(now + 60 * H));
        let cfg = WindowCfg::default();

        let mut logged = LoggedMap::new();
        logged.insert(1, prog(21, Some(200))); // remaining 179 → 48 h → excluded
        assert!(display_list(&[t.clone()], &logged, now, &cfg).is_empty());

        logged.insert(1, prog(20, Some(200))); // remaining 180 → 72 h → admitted
        assert_eq!(display_list(&[t], &logged, now, &cfg).len(), 1);
    }

    // No estimate → remaining 0 → 48 h horizon.
    #[test]
    fn no_estimate_uses_48h() {
        let now = 1_000_000;
        let cfg = WindowCfg::default();
        let logged = LoggedMap::new();
        let inside = task(1, Some(now + 47 * H));
        let outside = task(2, Some(now + 49 * H));
        let rows = display_list(&[inside, outside], &logged, now, &cfg);
        assert_eq!(rows.iter().map(|r| r.task_id).collect::<Vec<_>>(), vec![1]);
    }

    // Sort is deadline-ascending even when a later-deadline task was admitted by
    // a wider horizon than an earlier one.
    #[test]
    fn sorts_deadline_ascending_across_horizons() {
        let now = 1_000_000;
        let cfg = WindowCfg::default();
        let mut logged = LoggedMap::new();
        // Task A: due in 40 h, small remaining (48 h window).
        logged.insert(1, prog(0, Some(60)));
        // Task B: due in 100 h, big remaining (168 h window).
        logged.insert(2, prog(0, Some(800)));
        let a = task(1, Some(now + 40 * H));
        let b = task(2, Some(now + 100 * H));
        let rows = display_list(&[b, a], &logged, now, &cfg);
        assert_eq!(rows.iter().map(|r| r.task_id).collect::<Vec<_>>(), vec![1, 2]);
    }

    // NotStartedUrgent flips exactly at the 24 h threshold (default cfg).
    #[test]
    fn not_started_urgent_at_24h() {
        let now = 1_000_000;
        let cfg = WindowCfg::default();
        let logged = LoggedMap::new();

        // Exactly 24 h out → not urgent (strict `<`).
        let at = display_list(&[task(1, Some(now + 24 * H))], &logged, now, &cfg);
        assert_eq!(at[0].style, StyleClass::NotStarted);

        // One second under → urgent.
        let under = display_list(&[task(1, Some(now + 24 * H - 1))], &logged, now, &cfg);
        assert_eq!(under[0].style, StyleClass::NotStartedUrgent);
    }

    // Started rows get a completion band from logged/estimate.
    #[test]
    fn started_rows_banded() {
        let now = 1_000_000;
        let cfg = WindowCfg::default();
        let mut logged = LoggedMap::new();
        logged.insert(1, prog(10, Some(100))); // 0.10 → Band(0)
        logged.insert(2, prog(60, Some(100))); // 0.60 → Band(1)
        logged.insert(3, prog(95, Some(100))); // 0.95 → Band(2)
        let rows = display_list(
            &[task(1, Some(now + H)), task(2, Some(now + 2 * H)), task(3, Some(now + 3 * H))],
            &logged,
            now,
            &cfg,
        );
        assert_eq!(rows[0].style, StyleClass::Band(0));
        assert_eq!(rows[1].style, StyleClass::Band(1));
        assert_eq!(rows[2].style, StyleClass::Band(2));
    }

    // max_rows caps the list; the soonest deadlines survive the cut.
    #[test]
    fn caps_to_max_rows() {
        let now = 1_000_000;
        let cfg = WindowCfg { max_rows: 3, ..WindowCfg::default() };
        let logged = LoggedMap::new();
        let tasks: Vec<Task> = (1..=10).map(|i| task(i, Some(now + i * H))).collect();
        let rows = display_list(&tasks, &logged, now, &cfg);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.iter().map(|r| r.task_id).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    // Deadline-less tasks are never admitted (window keys on the deadline).
    #[test]
    fn deadline_less_excluded() {
        let now = 1_000_000;
        let cfg = WindowCfg::default();
        let logged = LoggedMap::new();
        assert!(display_list(&[task(1, None)], &logged, now, &cfg).is_empty());
    }
}
