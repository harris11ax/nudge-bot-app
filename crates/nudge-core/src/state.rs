//! Task-initiation state machine (pivot, session 8+). The svc's single armed
//! timer plus schedule/runtime edges drive every transition; this module is
//! pure — time enters only as `UnixTime`, effects leave only as data.
//!
//! Lifecycle of one nudge window:
//!   Idle --(window opens: TaskStart edge)--> Prompting --(escalate ladder)-->
//!   Prompting@L1/L2 ... --(Ack)--> Started --(check-in due)--> CheckIn
//!   --(Ack)--> Started ; any state --(window closes: WindowEnd)--> Idle.
//! Snooze hides the prompt and re-arms a SnoozeExpiry edge (still Prompting).
//! Skip dismisses the window for good (Started, no check-in).
//!
//! The state machine merges its own runtime edge (escalation step / snooze
//! expiry / check-in due) with the schedule edge from [`ScheduleCtx::next_edge`]
//! and arms the earliest — `ArmEdgeTimer` carries the winning [`EdgeKind`].

use crate::escalate::{evaluate, Ladder, Level};
use crate::{Edge, EdgeKind, Mode, Presence, UnixTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No active nudge: outside every window, or the current window was
    /// handled (started/skipped) — see [`State::Started`].
    Idle,
    /// A window is open and the user has not yet started the task. `shown_at`
    /// anchors the escalation ladder; `snooze_until` (when set and still in the
    /// future) means the prompt is temporarily hidden until that time. `mode` is
    /// picked once at the opening edge (UI-PLAN §1) and carried here so it stays
    /// stable across escalation ticks — the svc probes AW only at the edge.
    Prompting {
        shown_at: UnixTime,
        snooze_until: Option<UnixTime>,
        mode: Mode,
        /// `tasks` rowid backing this window (`None` for a rules `[[nudge]]`
        /// block). Picked once at the opening edge from
        /// [`ScheduleCtx::window_task_id`] and carried here so a notification
        /// click-through can target the task's page in nudge-app (9d-ii).
        task_id: Option<i64>,
    },
    /// The task was started (or skipped) for this window; no prompt shows.
    /// `checkin_at`, when set, is the time a "still on it?" check-in falls due.
    ///
    /// `sample_at` is the §6.1 STARTED-mode AW sample edge. It is `Some` only
    /// while sampling is configured and the task is live — a skipped window, a
    /// showing check-in, and every non-`Started` state carry no sample edge at
    /// all, which is how "no sampling when idle" is guaranteed structurally
    /// rather than by a guard: with no edge there is nothing to wake on.
    /// `off_task_since` is the start of the current *continuous* off-task run
    /// (`None` while on-task); once `now - off_task_since >= off_task_secs` the
    /// drift check-in fires (§6.5).
    Started {
        checkin_at: Option<UnixTime>,
        sample_at: Option<UnixTime>,
        off_task_since: Option<UnixTime>,
    },
    /// A post-start check-in prompt is showing, awaiting the user's answer.
    CheckIn { shown_at: UnixTime },
}

impl State {
    /// Stable label for the sessions log (Debug carries volatile timestamps).
    pub fn label(&self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Prompting { .. } => "prompting",
            State::Started { .. } => "started",
            State::CheckIn { .. } => "checkin",
        }
    }

    /// The `tasks` rowid backing the currently-shown prompt, if any — read by the
    /// svc on a notification click-through to target that task's page in
    /// nudge-app (9d-ii). Only a live `Prompting` window carries one; every other
    /// state (including a post-start check-in) yields `None`, i.e. open to the
    /// default page.
    pub fn task_id(&self) -> Option<i64> {
        match self {
            State::Prompting { task_id, .. } => *task_id,
            _ => None,
        }
    }
}

/// User's answer to a prompt, appended to the outcomes log by the svc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Started,
    Snoozed,
    Skipped,
    CheckedIn,
    /// A check-in that resolved itself because AW showed the user active — no
    /// prompt was ever shown. Logged distinctly from a manual `CheckedIn` so
    /// the outcomes log can tell "user answered" from "we let them be".
    AutoCheckedIn,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Started => "started",
            Outcome::Snoozed => "snoozed",
            Outcome::Skipped => "skipped",
            Outcome::CheckedIn => "checked_in",
            Outcome::AutoCheckedIn => "auto_checked_in",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The armed edge timer fired at this time (wake-and-recompute).
    EdgeTimer(UnixTime),
    /// Manual hotkey / tray toggle of the prompt.
    HotkeyToggle(UnixTime),
    /// Rules were reloaded (re-derive from current config).
    RulesReloaded(UnixTime),
    /// User pressed Start on the prompt.
    Ack(UnixTime),
    /// User pressed Snooze on the prompt.
    Snooze(UnixTime),
    /// User pressed Skip on the prompt.
    Skip(UnixTime),
}

impl Event {
    fn now(&self) -> UnixTime {
        match self {
            Event::EdgeTimer(t)
            | Event::HotkeyToggle(t)
            | Event::RulesReloaded(t)
            | Event::Ack(t)
            | Event::Snooze(t)
            | Event::Skip(t) => *t,
        }
    }
}

/// Effects are data; nudge-svc executes them. Every create effect has a paired
/// destroy effect emitted on the exiting transition (leak discipline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show (or update) the prompt overlay at the given visibility level and
    /// notification mode (OffTask strong/escalating vs. OnTask peripheral).
    ShowPrompt { text: String, level: Level, mode: Mode },
    /// Hide the prompt overlay.
    HidePrompt,
    /// Play the escalation alert sound (L2 re-alert).
    PlaySound,
    /// Arm the single waitable timer for the next edge (absolute time + kind).
    ArmEdgeTimer { at: UnixTime, kind: EdgeKind },
    /// Append a state-transition row to the sessions log. `mode` is set only on
    /// prompt-showing edges (Idle→Prompting, check-in) so the log records which
    /// notification mode was chosen (UI-PLAN §1 classification); `None` on
    /// non-prompt edges (idle/started).
    LogEdge { entered: &'static str, at: UnixTime, mode: Option<Mode> },
    /// Append a user-outcome row (ack/snooze/skip/check-in).
    LogOutcome { outcome: Outcome, at: UnixTime },
}

/// Caller-supplied context for `now`: schedule facts plus escalation/snooze/
/// check-in timing (sourced from rules; defaults until `[escalation]` lands).
pub struct ScheduleCtx {
    pub in_window: bool,
    pub window_text: String,
    /// `tasks` rowid of the open window (`None` for a rules `[[nudge]]` block or
    /// outside every window). Copied into `State::Prompting` at the opening edge.
    pub window_task_id: Option<i64>,
    /// `Task.mode_override` of the open window's task (`None` for a rules
    /// `[[nudge]]` block, a task with no override, or outside every window).
    /// When `Some`, the svc uses it verbatim at the task-start edge instead of
    /// classifying the foreground app (9f).
    pub window_mode_override: Option<Mode>,
    /// Next schedule edge (window start = TaskStart, end = WindowEnd).
    pub next_edge: Option<Edge>,
    pub ladder: Ladder,
    /// Snooze duration in seconds.
    pub snooze_secs: i64,
    /// Delay after Ack before a check-in falls due; `None` disables check-ins.
    pub checkin_after_secs: Option<i64>,
    /// STARTED-mode AW sampling cadence (§6.1); `None` disables sampling, in
    /// which case no `Started` state ever carries a `sample_at`.
    pub sample_secs: Option<i64>,
    /// Continuous off-task seconds that trigger the drift check-in (§6.5).
    pub off_task_secs: i64,
    /// Was the foreground app one of the live task's tools at a sample edge? The
    /// svc probes AW and does the set comparison; core only branches on the
    /// answer. `true` everywhere else (and when AW is down), so a missing signal
    /// never manufactures an off-task run.
    pub foreground_on_task: bool,
    /// Activity signal the svc probed from AW at a check-in edge (`Unknown`
    /// everywhere else). `Active` auto-resolves the check-in instead of nagging.
    pub presence: Presence,
    /// Notification mode for a prompt about to show (UI-PLAN §1). The svc picks
    /// it at the task-start edge from AW's foreground app vs. the productive-app
    /// list; `OffTask` everywhere else (schedule is activity-blind). Consumed by
    /// the two-mode overlay render in UI-P0/8b; the ladder only escalates in
    /// `OffTask`.
    pub mode: Mode,
}

/// Pure transition function.
pub fn next(state: State, event: &Event, ctx: &ScheduleCtx) -> (State, Vec<Effect>) {
    let now = event.now();
    let mut fx = Vec::new();

    // Manual toggle is orthogonal to the window lifecycle.
    if let Event::HotkeyToggle(_) = event {
        return hotkey_toggle(state, now, ctx);
    }

    // Window closed (or none defined): everything collapses to Idle.
    if !ctx.in_window {
        if !matches!(state, State::Idle) {
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogEdge { entered: "idle", at: now, mode: None });
        }
        arm_schedule(ctx, &mut fx);
        return (State::Idle, fx);
    }

    match (state, event) {
        // Window open while Idle → begin prompting (edge fired, or boot/reload
        // landed mid-window). shown_at = now starts the ladder fresh.
        (State::Idle, Event::EdgeTimer(_)) | (State::Idle, Event::RulesReloaded(_)) => {
            fx.push(Effect::LogEdge { entered: "prompting", at: now, mode: Some(ctx.mode) });
            drive_prompting(now, None, ctx.mode, ctx, now, &mut fx);
            (
                State::Prompting {
                    shown_at: now,
                    snooze_until: None,
                    mode: ctx.mode,
                    task_id: ctx.window_task_id,
                },
                fx,
            )
        }

        // Escalation tick / snooze resume / reload while prompting. `mode` rides
        // in the state (picked at the opening edge) — ctx.mode is only meaningful
        // on the Idle→Prompting edge, so re-read it from here, not from ctx.
        (State::Prompting { shown_at, snooze_until, mode, task_id }, Event::EdgeTimer(_))
        | (State::Prompting { shown_at, snooze_until, mode, task_id }, Event::RulesReloaded(_)) => {
            let (sa, su) = match snooze_until {
                // Snooze elapsed → re-show, restart the ladder from now.
                Some(until) if now >= until => (now, None),
                other => (shown_at, other),
            };
            drive_prompting(sa, su, mode, ctx, now, &mut fx);
            (
                State::Prompting {
                    shown_at: sa,
                    snooze_until: su,
                    mode,
                    task_id,
                },
                fx,
            )
        }

        // Start: the task goes live, so this is where the sampling spine spins up
        // (§6.1). `sample_at` exists from here until the window closes, the user
        // skips, or a check-in takes over.
        (State::Prompting { .. }, Event::Ack(_)) => {
            let checkin_at = ctx.checkin_after_secs.map(|s| now + s);
            let sample_at = ctx.sample_secs.map(|s| now + s);
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
            fx.push(Effect::LogOutcome {
                outcome: Outcome::Started,
                at: now,
            });
            arm_started(checkin_at, sample_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at,
                    sample_at,
                    off_task_since: None,
                },
                fx,
            )
        }

        (State::Prompting { shown_at, mode, task_id, .. }, Event::Snooze(_)) => {
            let until = now + ctx.snooze_secs;
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogOutcome {
                outcome: Outcome::Snoozed,
                at: now,
            });
            arm_merged(Some(Edge { at: until, kind: EdgeKind::SnoozeExpiry }), ctx, &mut fx);
            (
                State::Prompting {
                    shown_at,
                    snooze_until: Some(until),
                    mode,
                    task_id,
                },
                fx,
            )
        }

        // Skip dismisses the window for good: no check-in, and no sampling either
        // — the user said they aren't doing this, so watching them is noise.
        (State::Prompting { .. }, Event::Skip(_)) => {
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
            fx.push(Effect::LogOutcome {
                outcome: Outcome::Skipped,
                at: now,
            });
            arm_schedule(ctx, &mut fx);
            (
                State::Started {
                    checkin_at: None,
                    sample_at: None,
                    off_task_since: None,
                },
                fx,
            )
        }

        // A due edge (check-in or sample) fired, or we're just re-arming. Two
        // runtime edges now live in `Started`; both are merged into the single
        // timer by `arm_started`, and whichever is actually due drives the branch.
        (State::Started { checkin_at, sample_at, off_task_since }, Event::EdgeTimer(_))
        | (State::Started { checkin_at, sample_at, off_task_since }, Event::RulesReloaded(_)) => {
            // --- check-in edge (presence-informed, pre-Phase-3 behaviour) ---
            if let Some(t) = checkin_at {
                if now >= t {
                    // AW says the user is actively at the keyboard: they're plainly
                    // still working, so resolve it silently — no prompt, logged as
                    // an auto check-in — and settle back into Started with the
                    // sampling spine still running.
                    if ctx.presence == Presence::Active {
                        fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
                        fx.push(Effect::LogOutcome {
                            outcome: Outcome::AutoCheckedIn,
                            at: now,
                        });
                        // The sample edge may have come due on this same wake;
                        // advance it rather than re-arming a time already past.
                        let sample_at = bump_if_due(sample_at, now, ctx.sample_secs);
                        arm_started(None, sample_at, ctx, &mut fx);
                        return (
                            State::Started {
                                checkin_at: None,
                                sample_at,
                                off_task_since,
                            },
                            fx,
                        );
                    }
                    fx.push(Effect::ShowPrompt {
                        text: checkin_text(ctx),
                        level: Level::L0,
                        // A check-in is a discrete "still on it?" question — render
                        // it as an attention-getting OffTask prompt regardless of
                        // the window's own mode.
                        mode: Mode::OffTask,
                    });
                    fx.push(Effect::LogEdge { entered: "checkin", at: now, mode: Some(Mode::OffTask) });
                    arm_schedule(ctx, &mut fx);
                    return (State::CheckIn { shown_at: now }, fx);
                }
            }

            // --- sample edge (§6.1): is the user still in this task's tools? ---
            if let Some(sa) = sample_at {
                if now >= sa {
                    let next_sample = ctx.sample_secs.map(|s| now + s);
                    // On-task: nothing to say. Clear any part-built off-task run —
                    // the threshold measures a *continuous* drift, so returning to
                    // a tool resets it.
                    if ctx.foreground_on_task {
                        arm_started(checkin_at, next_sample, ctx, &mut fx);
                        return (
                            State::Started {
                                checkin_at,
                                sample_at: next_sample,
                                off_task_since: None,
                            },
                            fx,
                        );
                    }
                    // Off-task: the run starts at the first off-task sample and is
                    // measured from there, so the threshold is wall-clock drift,
                    // not a sample count.
                    let since = off_task_since.unwrap_or(now);
                    if now - since >= ctx.off_task_secs {
                        fx.push(Effect::ShowPrompt {
                            text: checkin_text(ctx),
                            level: Level::L0,
                            mode: Mode::OffTask,
                        });
                        fx.push(Effect::LogEdge { entered: "checkin", at: now, mode: Some(Mode::OffTask) });
                        arm_schedule(ctx, &mut fx);
                        return (State::CheckIn { shown_at: now }, fx);
                    }
                    arm_started(checkin_at, next_sample, ctx, &mut fx);
                    return (
                        State::Started {
                            checkin_at,
                            sample_at: next_sample,
                            off_task_since: Some(since),
                        },
                        fx,
                    );
                }
            }

            // --- nothing due: re-arm both edges unchanged ---
            arm_started(checkin_at, sample_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at,
                    sample_at,
                    off_task_since,
                },
                fx,
            )
        }

        // Answering a check-in returns to Started. Ack ("yes, still on it") resumes
        // the sampling spine with a fresh off-task run; Skip ("leave me alone")
        // stops it, matching Skip's dismiss-for-good meaning at the prompt.
        (State::CheckIn { .. }, Event::Ack(_)) | (State::CheckIn { .. }, Event::Skip(_)) => {
            let acked = matches!(event, Event::Ack(_));
            let outcome = if acked { Outcome::CheckedIn } else { Outcome::Skipped };
            let sample_at = if acked { ctx.sample_secs.map(|s| now + s) } else { None };
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
            fx.push(Effect::LogOutcome { outcome, at: now });
            arm_started(None, sample_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at: None,
                    sample_at,
                    off_task_since: None,
                },
                fx,
            )
        }

        // Reload while showing a check-in: re-show so a restart keeps it visible.
        (State::CheckIn { shown_at }, Event::RulesReloaded(_)) => {
            fx.push(Effect::ShowPrompt {
                text: checkin_text(ctx),
                level: Level::L0,
                mode: Mode::OffTask,
            });
            arm_schedule(ctx, &mut fx);
            (State::CheckIn { shown_at }, fx)
        }

        // Everything else in-window (e.g. stray user events with no live prompt,
        // EdgeTimer/Snooze while checking in): no change, just re-arm.
        _ => {
            arm_schedule(ctx, &mut fx);
            (state, fx)
        }
    }
}

/// Show the prompt and arm the next edge. While snoozed and not yet resumed,
/// stays hidden and arms the snooze-expiry edge. In `OffTask` the escalation
/// ladder drives the level, the re-alert sound, and an escalation-step edge;
/// in `OnTask` the prompt is a fixed peripheral L0 cue — no ladder, no sound,
/// no escalation edge (UI-PLAN §1: minimal interruption while already working).
fn drive_prompting(
    shown_at: UnixTime,
    snooze_until: Option<UnixTime>,
    mode: Mode,
    ctx: &ScheduleCtx,
    now: UnixTime,
    fx: &mut Vec<Effect>,
) {
    if let Some(until) = snooze_until {
        if now < until {
            arm_merged(Some(Edge { at: until, kind: EdgeKind::SnoozeExpiry }), ctx, fx);
            return;
        }
    }
    if mode == Mode::OnTask {
        fx.push(Effect::ShowPrompt {
            text: ctx.window_text.clone(),
            level: Level::L0,
            mode,
        });
        arm_schedule(ctx, fx);
        return;
    }
    let step = evaluate(&ctx.ladder, shown_at, now);
    fx.push(Effect::ShowPrompt {
        text: ctx.window_text.clone(),
        level: step.level,
        mode,
    });
    if step.realert {
        fx.push(Effect::PlaySound);
    }
    let runtime = step
        .next_step_at
        .map(|at| Edge { at, kind: EdgeKind::EscalationStep });
    arm_merged(runtime, ctx, fx);
}

/// Manual toggle: show a prompt when idle, hide any prompt otherwise.
fn hotkey_toggle(state: State, now: UnixTime, ctx: &ScheduleCtx) -> (State, Vec<Effect>) {
    let mut fx = Vec::new();
    match state {
        State::Idle => {
            fx.push(Effect::LogEdge { entered: "prompting", at: now, mode: Some(ctx.mode) });
            drive_prompting(now, None, ctx.mode, ctx, now, &mut fx);
            (
                State::Prompting {
                    shown_at: now,
                    snooze_until: None,
                    mode: ctx.mode,
                    task_id: ctx.window_task_id,
                },
                fx,
            )
        }
        _ => {
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogEdge { entered: "idle", at: now, mode: None });
            arm_schedule(ctx, &mut fx);
            (State::Idle, fx)
        }
    }
}

/// Curried [`Edge`] builder: `at.map(edge(kind))`.
fn edge(kind: EdgeKind) -> impl Fn(UnixTime) -> Edge {
    move |at| Edge { at, kind }
}

fn checkin_text(ctx: &ScheduleCtx) -> String {
    format!("still on: {}?", ctx.window_text)
}

/// Arm just the schedule edge (no competing runtime edge).
fn arm_schedule(ctx: &ScheduleCtx, fx: &mut Vec<Effect>) {
    arm_merged(None, ctx, fx);
}

/// Arm the single timer for a `Started` state, whose two runtime edges (check-in
/// and sample) collapse to their earliest before being merged with the schedule
/// edge. `Started` is the only state with two runtime candidates; funnelling them
/// through here keeps the one-armed-timer invariant a property of the code rather
/// than of every caller remembering it.
fn arm_started(
    checkin_at: Option<UnixTime>,
    sample_at: Option<UnixTime>,
    ctx: &ScheduleCtx,
    fx: &mut Vec<Effect>,
) {
    let runtime = earliest(
        checkin_at.map(edge(EdgeKind::CheckIn)),
        sample_at.map(edge(EdgeKind::Sample)),
    );
    arm_merged(runtime, ctx, fx);
}

/// The earlier of two optional edges (ties go to the first).
fn earliest(a: Option<Edge>, b: Option<Edge>) -> Option<Edge> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if x.at <= y.at { x } else { y }),
        (x, y) => x.or(y),
    }
}

/// Push an edge that has already come due out to the next interval, leaving a
/// not-yet-due edge (or a disabled one) alone. Without this, a wake that handles
/// one due edge would re-arm a sibling edge at a time already in the past and
/// spin an immediate second wake.
fn bump_if_due(at: Option<UnixTime>, now: UnixTime, secs: Option<i64>) -> Option<UnixTime> {
    match (at, secs) {
        (Some(a), Some(s)) if now >= a => Some(now + s),
        _ => at,
    }
}

/// Arm the earliest of a runtime edge and the schedule edge.
fn arm_merged(runtime: Option<Edge>, ctx: &ScheduleCtx, fx: &mut Vec<Effect>) {
    let pick = match (runtime, ctx.next_edge) {
        (Some(a), Some(b)) => Some(if a.at <= b.at { a } else { b }),
        (a, b) => a.or(b),
    };
    if let Some(e) = pick {
        fx.push(Effect::ArmEdgeTimer { at: e.at, kind: e.kind });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LADDER: Ladder = Ladder {
        l1_after_secs: 300,
        l2_after_secs: 600,
        l2_repeat_secs: 600,
    };

    /// Context builder. `end_at`/`start_at` control the schedule edge so tests
    /// can pit it against runtime edges.
    fn ctx(in_window: bool, edge: Option<Edge>) -> ScheduleCtx {
        ScheduleCtx {
            in_window,
            window_text: "focus".into(),
            window_task_id: None,
            window_mode_override: None,
            next_edge: edge,
            ladder: LADDER,
            snooze_secs: 600,
            checkin_after_secs: None,
            // Sampling off unless a test opts in, mirroring the rules default.
            sample_secs: None,
            off_task_secs: 300,
            foreground_on_task: true,
            presence: Presence::Unknown,
            mode: Mode::OffTask,
        }
    }

    /// A `Started` with sampling disabled — the shape every pre-Phase-3 test means
    /// when it says "started".
    fn started(checkin_at: Option<UnixTime>) -> State {
        State::Started {
            checkin_at,
            sample_at: None,
            off_task_since: None,
        }
    }

    fn window_end(at: UnixTime) -> Option<Edge> {
        Some(Edge { at, kind: EdgeKind::WindowEnd })
    }

    fn armed(fx: &[Effect]) -> Option<(UnixTime, EdgeKind)> {
        fx.iter().find_map(|e| match e {
            Effect::ArmEdgeTimer { at, kind } => Some((*at, *kind)),
            _ => None,
        })
    }

    #[test]
    fn window_opens_starts_prompting_at_l0() {
        let (s, fx) = next(State::Idle, &Event::EdgeTimer(1000), &ctx(true, window_end(9999)));
        assert!(matches!(s, State::Prompting { shown_at: 1000, snooze_until: None, .. }));
        assert!(fx.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L0, mode: Mode::OffTask }));
        assert!(fx.contains(&Effect::LogEdge { entered: "prompting", at: 1000, mode: Some(Mode::OffTask) }));
        // Escalation L1 edge (1300) beats the far-off window end.
        assert_eq!(armed(&fx), Some((1300, EdgeKind::EscalationStep)));
    }

    #[test]
    fn schedule_edge_wins_when_sooner_than_escalation() {
        // Window ends at 1100, before the L1 step at 1300 → arm WindowEnd.
        let (_, fx) = next(State::Idle, &Event::EdgeTimer(1000), &ctx(true, window_end(1100)));
        assert_eq!(armed(&fx), Some((1100, EdgeKind::WindowEnd)));
    }

    #[test]
    fn escalation_climbs_to_l2_and_realerts() {
        let st = State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None };
        // At 1300 → L1.
        let (s, fx) = next(st, &Event::EdgeTimer(1300), &ctx(true, window_end(9999)));
        assert!(fx.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L1, mode: Mode::OffTask }));
        assert!(!fx.contains(&Effect::PlaySound));
        // One repeat past L2 (2200) → re-alert with sound.
        let (_, fx2) = next(s, &Event::EdgeTimer(2200), &ctx(true, window_end(9999)));
        assert!(fx2.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L2, mode: Mode::OffTask }));
        assert!(fx2.contains(&Effect::PlaySound));
    }

    #[test]
    fn on_task_prompt_stays_l0_no_escalation_no_sound() {
        let mut c = ctx(true, window_end(9999));
        c.mode = Mode::OnTask;
        // Window opens on-task: peripheral L0 cue, no escalation edge armed
        // (only the far window end), no sound.
        let (s, fx) = next(State::Idle, &Event::EdgeTimer(1000), &c);
        assert!(matches!(s, State::Prompting { mode: Mode::OnTask, .. }));
        assert!(fx.contains(&Effect::ShowPrompt {
            text: "focus".into(),
            level: Level::L0,
            mode: Mode::OnTask,
        }));
        assert_eq!(armed(&fx), Some((9999, EdgeKind::WindowEnd)));
        // A later tick past the OffTask L2 boundary still shows L0, no sound.
        let (_, fx2) = next(s, &Event::EdgeTimer(2200), &c);
        assert!(fx2.contains(&Effect::ShowPrompt {
            text: "focus".into(),
            level: Level::L0,
            mode: Mode::OnTask,
        }));
        assert!(!fx2.contains(&Effect::PlaySound));
    }

    #[test]
    fn prompting_edge_logs_chosen_mode() {
        // Off-task opening logs mode=OffTask on the prompting edge.
        let (_, fx) = next(State::Idle, &Event::EdgeTimer(1000), &ctx(true, window_end(9999)));
        assert!(fx.contains(&Effect::LogEdge {
            entered: "prompting",
            at: 1000,
            mode: Some(Mode::OffTask),
        }));
        // On-task opening logs mode=OnTask.
        let mut c = ctx(true, window_end(9999));
        c.mode = Mode::OnTask;
        let (_, fx2) = next(State::Idle, &Event::EdgeTimer(1000), &c);
        assert!(fx2.contains(&Effect::LogEdge {
            entered: "prompting",
            at: 1000,
            mode: Some(Mode::OnTask),
        }));
        // A non-prompt edge (Ack → started) carries no mode.
        let st = State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None };
        let (_, fx3) = next(st, &Event::Ack(1200), &ctx(true, window_end(9999)));
        assert!(fx3.contains(&Effect::LogEdge { entered: "started", at: 1200, mode: None }));
    }

    #[test]
    fn prompting_carries_window_task_id_for_clickthrough() {
        let mut c = ctx(true, window_end(9999));
        c.window_task_id = Some(7);
        // Window opens: the task id rides into Prompting and is readable via the
        // svc-facing accessor.
        let (s, _) = next(State::Idle, &Event::EdgeTimer(1000), &c);
        assert!(matches!(s, State::Prompting { task_id: Some(7), .. }));
        assert_eq!(s.task_id(), Some(7));
        // It survives an escalation tick (ctx.window_task_id is only read at the
        // opening edge; the state carries it thereafter).
        let (s2, _) = next(s, &Event::EdgeTimer(1300), &ctx(true, window_end(9999)));
        assert_eq!(s2.task_id(), Some(7));
        // Non-prompt states expose no task id.
        assert_eq!(State::Idle.task_id(), None);
        assert_eq!(started(None).task_id(), None);
    }

    #[test]
    fn ack_starts_task_logs_outcome_no_checkin() {
        let st = State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None };
        let (s, fx) = next(st, &Event::Ack(1200), &ctx(true, window_end(5000)));
        assert_eq!(s, started(None));
        assert!(fx.contains(&Effect::HidePrompt));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::Started, at: 1200 }));
        assert_eq!(armed(&fx), Some((5000, EdgeKind::WindowEnd)));
    }

    #[test]
    fn ack_arms_checkin_when_enabled() {
        let mut c = ctx(true, window_end(9999));
        c.checkin_after_secs = Some(1800);
        let st = State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None };
        let (s, fx) = next(st, &Event::Ack(1200), &c);
        assert_eq!(s, started(Some(3000)));
        assert_eq!(armed(&fx), Some((3000, EdgeKind::CheckIn)));
    }

    #[test]
    fn checkin_fires_then_ack_returns_to_started() {
        let mut c = ctx(true, window_end(9999));
        c.checkin_after_secs = Some(1800);
        let st = started(Some(3000));
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert!(matches!(s, State::CheckIn { shown_at: 3000 }));
        assert!(fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        let (s2, fx2) = next(s, &Event::Ack(3100), &c);
        assert_eq!(s2, started(None));
        assert!(fx2.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 3100 }));
    }

    #[test]
    fn checkin_auto_resolves_when_user_active() {
        let mut c = ctx(true, window_end(9999));
        c.checkin_after_secs = Some(1800);
        c.presence = Presence::Active;
        let st = started(Some(3000));
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        // No prompt: the user is obviously working, so we back off to Started.
        assert_eq!(s, started(None));
        assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::AutoCheckedIn, at: 3000 }));
        assert_eq!(armed(&fx), Some((9999, EdgeKind::WindowEnd)));
    }

    #[test]
    fn checkin_still_prompts_when_away_or_unknown() {
        for p in [Presence::Away, Presence::Unknown] {
            let mut c = ctx(true, window_end(9999));
            c.checkin_after_secs = Some(1800);
            c.presence = p;
            let st = started(Some(3000));
            let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
            assert!(matches!(s, State::CheckIn { shown_at: 3000 }), "presence {p:?}");
            assert!(fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })), "presence {p:?}");
        }
    }

    #[test]
    fn snooze_hides_and_arms_expiry_then_resumes_fresh() {
        let st = State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None };
        let (s, fx) = next(st, &Event::Snooze(1200), &ctx(true, window_end(9999)));
        assert!(matches!(s, State::Prompting { snooze_until: Some(1800), .. }));
        assert!(fx.contains(&Effect::HidePrompt));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::Snoozed, at: 1200 }));
        assert_eq!(armed(&fx), Some((1800, EdgeKind::SnoozeExpiry)));
        // Timer fires before expiry: stay hidden, keep the same expiry edge.
        let (s2, fx2) = next(s, &Event::EdgeTimer(1500), &ctx(true, window_end(9999)));
        assert_eq!(s2, s);
        assert!(!fx2.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        assert_eq!(armed(&fx2), Some((1800, EdgeKind::SnoozeExpiry)));
        // At expiry: re-show at L0 with the ladder restarted from now.
        let (s3, fx3) = next(s2, &Event::EdgeTimer(1800), &ctx(true, window_end(9999)));
        assert!(matches!(s3, State::Prompting { shown_at: 1800, snooze_until: None, .. }));
        assert!(fx3.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L0, mode: Mode::OffTask }));
    }

    #[test]
    fn skip_dismisses_window_for_good() {
        let st = State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None };
        let (s, fx) = next(st, &Event::Skip(1200), &ctx(true, window_end(5000)));
        assert_eq!(s, started(None));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::Skipped, at: 1200 }));
        // A later timer in-window stays Started — no re-prompt.
        let (s2, fx2) = next(s, &Event::EdgeTimer(2000), &ctx(true, window_end(5000)));
        assert_eq!(s2, started(None));
        assert!(!fx2.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
    }

    #[test]
    fn window_close_collapses_to_idle_from_any_state() {
        for st in [
            State::Prompting { shown_at: 1000, snooze_until: None, mode: Mode::OffTask, task_id: None },
            started(Some(4000)),
            State::CheckIn { shown_at: 3000 },
        ] {
            let (s, fx) = next(
                st,
                &Event::EdgeTimer(5000),
                &ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart })),
            );
            assert_eq!(s, State::Idle);
            assert!(fx.contains(&Effect::HidePrompt));
            assert!(fx.contains(&Effect::LogEdge { entered: "idle", at: 5000, mode: None }));
            assert_eq!(armed(&fx), Some((90_000, EdgeKind::TaskStart)));
        }
    }

    #[test]
    fn idle_out_of_window_is_quiet() {
        let (s, fx) = next(
            State::Idle,
            &Event::EdgeTimer(5000),
            &ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart })),
        );
        assert_eq!(s, State::Idle);
        assert!(!fx.iter().any(|e| matches!(e, Effect::HidePrompt | Effect::ShowPrompt { .. })));
        assert_eq!(fx, vec![Effect::ArmEdgeTimer { at: 90_000, kind: EdgeKind::TaskStart }]);
    }

    // --- §6.1 STARTED sampling spine ---

    /// Context with sampling live: 5-min cadence, 5-min off-task threshold.
    fn sampling_ctx(edge: Option<Edge>) -> ScheduleCtx {
        let mut c = ctx(true, edge);
        c.sample_secs = Some(300);
        c.off_task_secs = 300;
        c
    }

    fn prompting() -> State {
        State::Prompting {
            shown_at: 1000,
            snooze_until: None,
            mode: Mode::OffTask,
            task_id: None,
        }
    }

    // Ack spins the spine up: Started carries a sample edge one cadence out, and
    // the timer arms it because it beats the far-off window end.
    #[test]
    fn ack_arms_sample_edge_when_sampling_enabled() {
        let (s, fx) = next(prompting(), &Event::Ack(1200), &sampling_ctx(window_end(9999)));
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(1500),
                off_task_since: None
            }
        );
        assert_eq!(armed(&fx), Some((1500, EdgeKind::Sample)));
    }

    // The two Started runtime edges collapse to their earliest before meeting the
    // schedule edge — one armed timer, never two.
    #[test]
    fn started_arms_earliest_of_checkin_sample_and_schedule() {
        let mut c = sampling_ctx(window_end(9999));
        c.checkin_after_secs = Some(120); // check-in at 1320 beats sample at 1500
        let (_, fx) = next(prompting(), &Event::Ack(1200), &c);
        assert_eq!(armed(&fx), Some((1320, EdgeKind::CheckIn)));

        // Sample sooner than the check-in → sample wins.
        c.checkin_after_secs = Some(3600);
        let (_, fx) = next(prompting(), &Event::Ack(1200), &c);
        assert_eq!(armed(&fx), Some((1500, EdgeKind::Sample)));

        // Window closing before either runtime edge → the schedule edge wins.
        let mut c2 = sampling_ctx(window_end(1400));
        c2.checkin_after_secs = Some(3600);
        let (_, fx) = next(prompting(), &Event::Ack(1200), &c2);
        assert_eq!(armed(&fx), Some((1400, EdgeKind::WindowEnd)));

        // Exactly one timer is armed on every one of those transitions.
        assert_eq!(
            fx.iter().filter(|e| matches!(e, Effect::ArmEdgeTimer { .. })).count(),
            1
        );
    }

    // An on-task sample is silent: re-arm one cadence out, stay Started, no prompt.
    #[test]
    fn on_task_sample_rearms_and_stays_quiet() {
        let c = sampling_ctx(window_end(9999)); // foreground_on_task defaults true
        let st = State::Started {
            checkin_at: None,
            sample_at: Some(1500),
            off_task_since: None,
        };
        let (s, fx) = next(st, &Event::EdgeTimer(1500), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(1800),
                off_task_since: None
            }
        );
        assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        assert_eq!(armed(&fx), Some((1800, EdgeKind::Sample)));
    }

    // A continuous off-task run: the first sample opens the run, later samples
    // carry it, and crossing off_task_secs raises the drift check-in.
    #[test]
    fn off_task_run_crosses_threshold_and_raises_checkin() {
        let mut c = sampling_ctx(window_end(9999));
        c.foreground_on_task = false;
        let st = State::Started {
            checkin_at: None,
            sample_at: Some(1500),
            off_task_since: None,
        };

        // First off-task sample: run opens at 1500, still under threshold.
        let (s, fx) = next(st, &Event::EdgeTimer(1500), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(1800),
                off_task_since: Some(1500)
            }
        );
        assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));

        // Second sample at 1800: the run is exactly 300s → threshold crossed.
        let (s2, fx2) = next(s, &Event::EdgeTimer(1800), &c);
        assert!(matches!(s2, State::CheckIn { shown_at: 1800 }));
        assert!(fx2.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        assert!(fx2.contains(&Effect::LogEdge {
            entered: "checkin",
            at: 1800,
            mode: Some(Mode::OffTask)
        }));
        // A showing check-in owns the screen: no sample edge competes with it.
        assert_eq!(armed(&fx2), Some((9999, EdgeKind::WindowEnd)));
    }

    // Returning to a tool resets the run — the threshold measures *continuous*
    // drift, so an off-task blip never accumulates toward a check-in.
    #[test]
    fn returning_on_task_resets_the_off_task_run() {
        let mut c = sampling_ctx(window_end(9999));
        c.foreground_on_task = false;
        let st = State::Started {
            checkin_at: None,
            sample_at: Some(1500),
            off_task_since: None,
        };
        let (s, _) = next(st, &Event::EdgeTimer(1500), &c);
        assert!(matches!(s, State::Started { off_task_since: Some(1500), .. }));

        // Back on-task at the next sample: run cleared.
        c.foreground_on_task = true;
        let (s2, _) = next(s, &Event::EdgeTimer(1800), &c);
        assert!(matches!(s2, State::Started { off_task_since: None, .. }));

        // Off-task again at 2100 opens a *fresh* run, so 2400 is only 300s in and
        // the earlier blip contributes nothing.
        c.foreground_on_task = false;
        let (s3, _) = next(s2, &Event::EdgeTimer(2100), &c);
        assert!(matches!(s3, State::Started { off_task_since: Some(2100), .. }));
    }

    // AW down (or any edge with no foreground probe) reads as on-task: the user is
    // left alone rather than nagged on a signal we don't have.
    #[test]
    fn missing_foreground_signal_never_manufactures_drift() {
        let c = sampling_ctx(window_end(9999)); // foreground_on_task: true default
        let st = State::Started {
            checkin_at: None,
            sample_at: Some(1500),
            off_task_since: Some(1000), // a run that would otherwise be long past due
        };
        let (s, fx) = next(st, &Event::EdgeTimer(1500), &c);
        assert!(matches!(s, State::Started { off_task_since: None, .. }));
        assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
    }

    // The zero-polling guarantee (PLAN §7): no state outside Started-with-sampling
    // can arm a Sample edge, because none of them carries a sample_at to arm.
    #[test]
    fn no_sample_edge_outside_started() {
        let c = sampling_ctx(window_end(9999));
        let armed_kind = |fx: &[Effect]| armed(fx).map(|(_, k)| k);

        // Idle out of window.
        let out = ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart }));
        let (_, fx) = next(State::Idle, &Event::EdgeTimer(5000), &out);
        assert_ne!(armed_kind(&fx), Some(EdgeKind::Sample));

        // Prompting (window open, task not started yet).
        let (s, fx) = next(State::Idle, &Event::EdgeTimer(1000), &c);
        assert!(matches!(s, State::Prompting { .. }));
        assert_ne!(armed_kind(&fx), Some(EdgeKind::Sample));

        // A showing check-in.
        let (_, fx) = next(State::CheckIn { shown_at: 3000 }, &Event::EdgeTimer(3100), &c);
        assert_ne!(armed_kind(&fx), Some(EdgeKind::Sample));

        // Sampling disabled by config: Started itself carries no sample edge.
        let (s, fx) = next(prompting(), &Event::Ack(1200), &ctx(true, window_end(9999)));
        assert!(matches!(s, State::Started { sample_at: None, .. }));
        assert_ne!(armed_kind(&fx), Some(EdgeKind::Sample));

        // Window closing collapses a sampling Started to Idle — the edge is gone,
        // not merely skipped.
        let live = State::Started {
            checkin_at: None,
            sample_at: Some(1500),
            off_task_since: None,
        };
        let (s, fx) = next(live, &Event::EdgeTimer(5000), &out);
        assert_eq!(s, State::Idle);
        assert_eq!(armed(&fx), Some((90_000, EdgeKind::TaskStart)));
    }

    // Skip means "not doing this": no check-in and no sampling either.
    #[test]
    fn skip_starts_no_sampling() {
        let (s, fx) = next(prompting(), &Event::Skip(1200), &sampling_ctx(window_end(5000)));
        assert_eq!(s, started(None));
        assert_eq!(armed(&fx), Some((5000, EdgeKind::WindowEnd)));
    }

    // An auto-resolved check-in leaves the spine running, and advances a sample
    // edge that came due on the same wake instead of re-arming it in the past.
    #[test]
    fn auto_checkin_keeps_sampling_alive() {
        let mut c = sampling_ctx(window_end(9999));
        c.presence = Presence::Active;
        let st = State::Started {
            checkin_at: Some(3000),
            sample_at: Some(3000), // due on this same wake
            off_task_since: None,
        };
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(3300), // bumped, not left at 3000
                off_task_since: None
            }
        );
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::AutoCheckedIn, at: 3000 }));
        assert_eq!(armed(&fx), Some((3300, EdgeKind::Sample)));
    }

    // Answering the drift check-in: Ack resumes sampling with a clean run; Skip
    // stops it.
    #[test]
    fn checkin_answer_resumes_or_stops_sampling() {
        let c = sampling_ctx(window_end(9999));
        let (s, fx) = next(State::CheckIn { shown_at: 1800 }, &Event::Ack(1900), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(2200),
                off_task_since: None
            }
        );
        assert_eq!(armed(&fx), Some((2200, EdgeKind::Sample)));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 1900 }));

        let (s2, fx2) = next(State::CheckIn { shown_at: 1800 }, &Event::Skip(1900), &c);
        assert_eq!(s2, started(None));
        assert_eq!(armed(&fx2), Some((9999, EdgeKind::WindowEnd)));
        assert!(fx2.contains(&Effect::LogOutcome { outcome: Outcome::Skipped, at: 1900 }));
    }

    // A wake with nothing due re-arms both edges untouched (no drift in the run,
    // no premature sample).
    #[test]
    fn started_wake_with_nothing_due_rearms_unchanged() {
        let c = sampling_ctx(window_end(9999));
        let st = State::Started {
            checkin_at: Some(4000),
            sample_at: Some(1500),
            off_task_since: Some(1400),
        };
        let (s, fx) = next(st, &Event::EdgeTimer(1200), &c);
        assert_eq!(s, st);
        assert_eq!(armed(&fx), Some((1500, EdgeKind::Sample)));
    }

    #[test]
    fn hotkey_toggles_prompt_on_and_off() {
        let (s, fx) = next(State::Idle, &Event::HotkeyToggle(50), &ctx(true, window_end(9999)));
        assert!(matches!(s, State::Prompting { .. }));
        assert!(fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        let (s2, fx2) = next(s, &Event::HotkeyToggle(60), &ctx(true, window_end(9999)));
        assert_eq!(s2, State::Idle);
        assert!(fx2.contains(&Effect::HidePrompt));
    }
}
