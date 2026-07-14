//! Given rules + now, answer two questions with zero tick logic:
//! (1) is `now` inside a nudge window (and which)? (2) what is the single
//! next edge timestamp the svc must arm a timer for?

use crate::rules::{parse_hhmm, NudgeWindow, Rules};
use crate::state::ScheduleCtx;
use crate::tasks::{Recur, Task};
use crate::{Edge, EdgeKind, Mode, Presence, UnixTime};

/// Local-time snapshot injected by svc (core never reads the clock or TZ).
#[derive(Debug, Clone, Copy)]
pub struct LocalNow {
    pub unix: UnixTime,
    /// 0 = mon .. 6 = sun.
    pub weekday: u8,
    /// Minutes since local midnight.
    pub minutes: u32,
}

const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// Minutes a task-derived nudge window stays open. Tasks are *initiation*
/// triggers (a start time, no explicit end — unlike a rules `[[nudge]]` block),
/// so the scheduler synthesizes an end at `start + TASK_WINDOW_MINUTES`, giving
/// the escalation ladder room to run before `WindowEnd` drops the prompt.
const TASK_WINDOW_MINUTES: u32 = 30;

/// A nudge window normalized for scheduling, independent of its source (a rules
/// `[[nudge]]` block or a recurring `tasks` row). `active[i]` marks weekday
/// `i` (0 = mon .. 6 = sun); `start`/`end` are minutes since local midnight
/// (`end` may exceed 1439 for a window that spills past midnight).
struct Win {
    active: [bool; 7],
    /// For a `Recur::Once` task: the unix timestamp whose local date the window
    /// fires on. When `Some`, `active` is ignored and the window is live only on
    /// the single day whose midnight..+24h contains this instant — so a one-shot
    /// task fires exactly once (the date passes) with no completion flag needed,
    /// and a past-dated once task contributes no future edge. `None` for weekly
    /// windows, which use `active` as before.
    on_date: Option<UnixTime>,
    start: u32,
    end: u32,
    text: String,
    /// `tasks` rowid this window came from, or `None` for a rules `[[nudge]]`
    /// block. Threaded to [`ScheduleCtx::window_task_id`] so a notification
    /// click-through can target the task's page in nudge-app (9d-ii).
    task_id: Option<i64>,
    /// The task's `mode_override`, or `None` for a rules `[[nudge]]` block or a
    /// task with no override. Threaded to [`ScheduleCtx::window_mode_override`]
    /// so svc can skip edge classification when a task pins its own mode (9f).
    mode_override: Option<Mode>,
}

impl Win {
    /// Normalize a rules `[[nudge]]` block. `unwrap`s are safe: `rules::parse`
    /// already validated the day names and `HH:MM` fields.
    fn from_nudge(w: &NudgeWindow) -> Win {
        let mut active = [false; 7];
        for (i, d) in DAYS.iter().enumerate() {
            if w.days.iter().any(|x| x == d) {
                active[i] = true;
            }
        }
        Win {
            active,
            on_date: None,
            start: parse_hhmm(&w.start).unwrap(),
            end: parse_hhmm(&w.end).unwrap(),
            text: w.text.clone(),
            // A rules block is not a task — no id to hand the app, no override.
            task_id: None,
            mode_override: None,
        }
    }

    /// Normalize a `tasks` row, or `None` when it can't yet drive a window:
    /// a deadline-only task (`minutes == None`, no time-of-day cue) or a
    /// `Recur::Once` task (no weekday anchor — one-shot firing needs a
    /// completion flag the read-only svc can't set. A `Recur::Once` task with a
    /// deadline is date-anchored instead: it fires on the deadline's calendar
    /// date and never again (the date passes), so no completion flag is needed.
    /// A `Recur::Once` task with no deadline stays deferred (nothing to anchor
    /// the single firing to). Weekly tasks map directly, like a `[[nudge]]` block.
    fn from_task(t: &Task) -> Option<Win> {
        let start = t.minutes?;
        let (active, on_date) = match &t.recur {
            Recur::Weekly(days) => {
                let mut active = [false; 7];
                for d in days {
                    active[*d as usize] = true;
                }
                (active, None)
            }
            Recur::Once => ([false; 7], Some(t.deadline?)),
        };
        Some(Win {
            active,
            on_date,
            start,
            end: start + TASK_WINDOW_MINUTES,
            text: t.title.clone(),
            task_id: t.id,
            mode_override: t.mode_override,
        })
    }
}

/// Build the transition context for `now` from the rules `[[nudge]]` blocks
/// only. See [`context_with_tasks`] for the app-created `tasks`-table triggers.
pub fn context(rules: &Rules, now: LocalNow) -> ScheduleCtx {
    context_with_tasks(rules, &[], now)
}

/// Build the transition context for `now` from both the rules `[[nudge]]` blocks
/// and the app-created `tasks` rows (the svc reads them read-only and passes
/// them here — closing the app-writer/svc-reader loop). `next_edge` is the
/// earliest window start/end strictly after now, searched up to 7 days ahead
/// (None if no windows exist at all). A window start is tagged
/// [`EdgeKind::TaskStart`] (begin prompting), a window end [`EdgeKind::WindowEnd`]
/// (drop the prompt). When a rules window and a task window are both active,
/// the task's text wins (it's the more specific, user-created trigger); its
/// `mode_override` (if set) is exposed the same way via
/// [`ScheduleCtx::window_mode_override`] (9f).
pub fn context_with_tasks(rules: &Rules, tasks: &[Task], now: LocalNow) -> ScheduleCtx {
    // Rules windows first, then tasks, so an overlapping task's text wins the
    // last-match assignment below.
    let mut wins: Vec<Win> = rules.nudges.iter().map(Win::from_nudge).collect();
    wins.extend(tasks.iter().filter_map(Win::from_task));

    let mut in_window = false;
    let mut text = rules.anchor.default_text.clone();
    let mut window_task_id: Option<i64> = None;
    let mut window_mode_override: Option<Mode> = None;
    let mut best: Option<Edge> = None;

    // 0..=7: day 7 catches a once-a-week window whose edges already passed today.
    for day_off in 0..8u8 {
        let wd = (now.weekday + day_off) % 7;
        let midnight = now.unix - (now.minutes as i64) * 60 + (day_off as i64) * 86_400;
        for w in &wins {
            let active_today = match w.on_date {
                // Once task: live only on the day whose local date contains the
                // deadline instant (this day_off's midnight..+24h).
                Some(d) => d >= midnight && d < midnight + 86_400,
                None => w.active[wd as usize],
            };
            if !active_today {
                continue;
            }
            let (s, e) = (w.start, w.end);
            if day_off == 0 && now.minutes >= s && now.minutes < e {
                in_window = true;
                text = w.text.clone();
                // Same last-match-wins order as `text`: an overlapping task
                // window (appended after the rules blocks) carries its id here,
                // while a rules-only window leaves it `None`.
                window_task_id = w.task_id;
                window_mode_override = w.mode_override;
            }
            for (edge_min, kind) in [(s, EdgeKind::TaskStart), (e, EdgeKind::WindowEnd)] {
                let t = midnight + (edge_min as i64) * 60;
                if t > now.unix && best.is_none_or(|b| t < b.at) {
                    best = Some(Edge { at: t, kind });
                }
            }
        }
    }
    ScheduleCtx {
        in_window,
        window_text: text,
        window_task_id,
        window_mode_override,
        next_edge: best,
        // Escalation/snooze/check-in timing from the `[escalation]` block (all
        // fields default, so an absent block yields the launch defaults); only
        // these three feed the runtime edges.
        ladder: rules.escalation.ladder(),
        snooze_secs: rules.escalation.snooze_secs,
        checkin_after_secs: rules.escalation.checkin(),
        // Schedule knows nothing of live activity; the svc overrides these at the
        // relevant edge after probing AW (presence at check-in, mode at task-start).
        presence: Presence::Unknown,
        mode: Mode::OffTask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::parse;

    const R: &str = r#"
[anchor]
default_text = "idle"
[[nudge]]
name = "a"
days = ["mon"]
start = "08:00"
end = "10:00"
text = "work"
"#;

    // Monday, 09:00 local, midnight at unix 0 for simplicity.
    fn at(weekday: u8, minutes: u32) -> LocalNow {
        LocalNow {
            unix: (minutes as i64) * 60,
            weekday,
            minutes,
        }
    }

    #[test]
    fn inside_window() {
        let ctx = context(&parse(R).unwrap(), at(0, 9 * 60));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_text, "work");
        // today's end edge, tagged as the window closing.
        assert_eq!(
            ctx.next_edge,
            Some(Edge { at: 10 * 3600, kind: EdgeKind::WindowEnd })
        );
    }

    #[test]
    fn before_window_next_edge_is_start() {
        let ctx = context(&parse(R).unwrap(), at(0, 7 * 60));
        assert!(!ctx.in_window);
        assert_eq!(
            ctx.next_edge,
            Some(Edge { at: 8 * 3600, kind: EdgeKind::TaskStart })
        );
    }

    #[test]
    fn after_window_wraps_to_next_week() {
        let ctx = context(&parse(R).unwrap(), at(0, 11 * 60));
        assert!(!ctx.in_window);
        assert_eq!(
            ctx.next_edge,
            Some(Edge { at: 7 * 86_400 + 8 * 3600, kind: EdgeKind::TaskStart })
        );
    }

    #[test]
    fn no_windows_no_edge() {
        let empty = parse("[anchor]\ndefault_text = \"x\"\n").unwrap();
        assert_eq!(context(&empty, at(0, 0)).next_edge, None);
    }

    use crate::tasks::{Recur, Task, TriggerSource};

    fn task(minutes: Option<u32>, recur: Recur, title: &str) -> Task {
        Task {
            id: Some(1),
            title: title.to_string(),
            desc: String::new(),
            deadline: None,
            task_type: String::new(),
            minutes,
            recur,
            mode_override: None,
            trigger_source: TriggerSource::Manual,
            gcal_event_id: None,
        }
    }

    // A weekly task fires like a `[[nudge]]` block: opens at its minute, closes
    // TASK_WINDOW_MINUTES later, and its title becomes the window text.
    #[test]
    fn weekly_task_produces_window() {
        // No rules windows — only the task drives the schedule.
        let empty = parse("[anchor]\ndefault_text = \"idle\"\n").unwrap();
        let t = task(Some(9 * 60), Recur::Weekly(vec![0]), "gym");

        // Monday 09:10, inside the 09:00–09:30 synthesized window.
        let ctx = context_with_tasks(&empty, std::slice::from_ref(&t), at(0, 9 * 60 + 10));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_text, "gym");
        assert_eq!(
            ctx.next_edge,
            Some(Edge {
                at: (9 * 60 + 30) as i64 * 60,
                kind: EdgeKind::WindowEnd
            })
        );

        // Monday 08:00, before it opens: next edge is the task start.
        let ctx = context_with_tasks(&empty, std::slice::from_ref(&t), at(0, 8 * 60));
        assert!(!ctx.in_window);
        assert_eq!(
            ctx.next_edge,
            Some(Edge { at: 9 * 3600, kind: EdgeKind::TaskStart })
        );
    }

    // A deadline-only task (no time-of-day) and a Once task with no deadline
    // (nothing to anchor the single firing to) contribute no edges.
    #[test]
    fn deadline_only_and_undated_once_skipped() {
        let empty = parse("[anchor]\ndefault_text = \"idle\"\n").unwrap();
        let undated_once = task(Some(9 * 60), Recur::Once, "call mum"); // deadline None
        let deadline_only = task(None, Recur::Weekly(vec![0]), "report");
        let ctx = context_with_tasks(&empty, &[undated_once, deadline_only], at(0, 9 * 60 + 10));
        assert!(!ctx.in_window);
        assert_eq!(ctx.next_edge, None);
    }

    // A Once task dated today fires: its window opens at `minutes` on the
    // deadline's date and closes TASK_WINDOW_MINUTES later.
    #[test]
    fn dated_once_task_fires_on_its_date() {
        let empty = parse("[anchor]\ndefault_text = \"idle\"\n").unwrap();
        // Monday midnight is unix 0 in this harness (at() sets unix = minutes*60).
        // Deadline anywhere within Monday anchors the fire to Monday.
        let mut once = task(Some(9 * 60), Recur::Once, "call mum");
        once.deadline = Some(9 * 3600); // Monday 09:00

        // Monday 09:10 — inside the 09:00–09:30 synthesized window.
        let ctx = context_with_tasks(&empty, std::slice::from_ref(&once), at(0, 9 * 60 + 10));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_text, "call mum");
        assert_eq!(
            ctx.next_edge,
            Some(Edge { at: (9 * 60 + 30) as i64 * 60, kind: EdgeKind::WindowEnd })
        );

        // Monday 08:00 — before it opens: next edge is the task start.
        let ctx = context_with_tasks(&empty, std::slice::from_ref(&once), at(0, 8 * 60));
        assert!(!ctx.in_window);
        assert_eq!(
            ctx.next_edge,
            Some(Edge { at: 9 * 3600, kind: EdgeKind::TaskStart })
        );
    }

    // A Once task whose date already passed contributes no future edge — it fired
    // once and won't again, no completion flag required.
    #[test]
    fn past_dated_once_task_contributes_no_edge() {
        let empty = parse("[anchor]\ndefault_text = \"idle\"\n").unwrap();
        let mut once = task(Some(9 * 60), Recur::Once, "call mum");
        once.deadline = Some(9 * 3600); // Monday 09:00
        // Now is Tuesday 10:00 (unix = 1 day + 10h); Monday is in the past.
        let now = LocalNow { unix: 86_400 + 10 * 3600, weekday: 1, minutes: 10 * 60 };
        let ctx = context_with_tasks(&empty, std::slice::from_ref(&once), now);
        assert!(!ctx.in_window);
        assert_eq!(ctx.next_edge, None);
    }

    // An overlapping task window wins the text over a rules `[[nudge]]` block.
    #[test]
    fn task_text_wins_over_rules_window() {
        // R defines mon 08:00–10:00 "work"; task overlaps at 09:00–09:30 "gym".
        let t = task(Some(9 * 60), Recur::Weekly(vec![0]), "gym");
        let ctx = context_with_tasks(&parse(R).unwrap(), std::slice::from_ref(&t), at(0, 9 * 60 + 5));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_text, "gym");
    }

    // A task window exposes its rowid via `window_task_id` (9d-ii click-through
    // targeting); an overlapping task's id wins over the rules window just like
    // its text, and a rules-only window carries no id.
    #[test]
    fn task_window_threads_its_id() {
        let mut t = task(Some(9 * 60), Recur::Weekly(vec![0]), "gym");
        t.id = Some(42);
        // Inside the task window: its id is exposed.
        let ctx = context_with_tasks(&parse(R).unwrap(), std::slice::from_ref(&t), at(0, 9 * 60 + 5));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_task_id, Some(42));

        // Inside the rules `[[nudge]]` window but before the task opens (08:30):
        // rules-only window, so no task id.
        let ctx = context_with_tasks(&parse(R).unwrap(), std::slice::from_ref(&t), at(0, 8 * 60 + 30));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_text, "work");
        assert_eq!(ctx.window_task_id, None);
    }

    // A task's `mode_override` rides alongside its id (9f): exposed while its
    // window is open, `None` for a rules-only window, `None` when the task sets
    // no override.
    #[test]
    fn task_window_threads_its_mode_override() {
        let mut t = task(Some(9 * 60), Recur::Weekly(vec![0]), "gym");
        t.mode_override = Some(Mode::OnTask);

        let ctx = context_with_tasks(&parse(R).unwrap(), std::slice::from_ref(&t), at(0, 9 * 60 + 5));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_mode_override, Some(Mode::OnTask));

        // Rules-only window (before the task opens): no override.
        let ctx = context_with_tasks(&parse(R).unwrap(), std::slice::from_ref(&t), at(0, 8 * 60 + 30));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_mode_override, None);

        // Task window open but no override set on the task: still None.
        let t_no_override = task(Some(9 * 60), Recur::Weekly(vec![0]), "gym");
        let ctx = context_with_tasks(&parse(R).unwrap(), std::slice::from_ref(&t_no_override), at(0, 9 * 60 + 5));
        assert!(ctx.in_window);
        assert_eq!(ctx.window_mode_override, None);
    }
}
