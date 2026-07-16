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
use crate::task_window::Row;
use crate::{Edge, EdgeKind, Mode, Presence, UnixTime};

/// Why a check-in is on screen — it decides which answer buttons make sense and
/// what a "no" means (§6.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckInKind {
    /// The post-ack "still on it?" check-in raised by `checkin_at`. Presence
    /// answers it silently when it can; otherwise Start/Skip do.
    Periodic,
    /// §6.5 drift check-in: a continuous off-task run crossed `off_task_secs`.
    /// Answered Yes/No — No brings the task list.
    OffTask,
    /// §6.4 on-task check-in: floor-cadence "what are you working on?" raised
    /// when the user has drifted off EVERY due-window task's tools at an
    /// `ontask_at` tick. Answered by picking a task (→ classification, Tier-B
    /// P2), not Yes/No.
    OnTask,
}

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
        /// §6.4 on-task check-in cadence edge. `Some(t)` = next floor tick at
        /// which, if the foreground app is off EVERY due-window task's tools
        /// (`!ctx.any_task_on_task`), we raise `CheckInKind::OnTask`. `None`
        /// when the cadence is disabled (`ctx.ontask_secs == None`) or the
        /// window was skipped. This is the THIRD runtime edge; it collapses
        /// into `arm_started`'s min-merge so the single-armed-timer invariant
        /// (PLAN-step1 §6.1) still holds structurally.
        ontask_at: Option<UnixTime>,
    },
    /// A post-start check-in prompt is showing, awaiting the user's answer.
    CheckIn { shown_at: UnixTime, kind: CheckInKind },
    /// The user answered No to a drift check-in and the §6.5 task list is on
    /// screen. Modelled as part of the check-in family: an ignored list dismisses
    /// itself at the next schedule edge, exactly as an ignored check-in does.
    Choosing { shown_at: UnixTime },
    /// Tray Pause (§6.6). Reachable from any state; everything is off screen and
    /// exactly one `PauseExpiry` edge is armed — no schedule edge is merged, so
    /// nothing fires until `resume_at`. `was_started` remembers whether a task was
    /// live, so resuming lands back in `Started` instead of re-nagging the user to
    /// start what they were already doing.
    Paused { resume_at: UnixTime, was_started: bool },
    /// Take-a-break from the §6.5 No path. Identical single-edge silencing to
    /// [`State::Paused`]; distinct only so the outcomes log can tell a break the
    /// user chose from a pause they reached for.
    Break { resume_at: UnixTime },
    /// The Tier-B P2 classification screen is up: each tool seen since the last
    /// check-in is being routed to `task_id`'s tool list / the global not-tool
    /// list / a task-scoped ignore. Check-in-family: an ignored screen dies at
    /// the next schedule edge like an ignored [`State::Choosing`]. Core never
    /// holds the tool list — the svc snapshotted it into the `ShowClassify`
    /// effect and persists each choice itself; core only waits for
    /// [`Event::ClassifyDone`].
    Classifying { task_id: i64, shown_at: UnixTime },
}

impl State {
    /// Stable label for the sessions log (Debug carries volatile timestamps).
    pub fn label(&self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::Prompting { .. } => "prompting",
            State::Started { .. } => "started",
            State::CheckIn { .. } => "checkin",
            State::Choosing { .. } => "choosing",
            State::Paused { .. } => "paused",
            State::Break { .. } => "break",
            State::Classifying { .. } => "classifying",
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
    /// "No, I'm not on that" at a drift check-in (§6.5) — the answer that brings
    /// the task list up.
    CheckedInNo,
    /// Tray Pause (§6.6).
    Paused,
    /// Take-a-break chosen from the §6.5 list.
    BreakTaken,
    /// A pause or break ended (expired or resumed early).
    Resumed,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Started => "started",
            Outcome::Snoozed => "snoozed",
            Outcome::Skipped => "skipped",
            Outcome::CheckedIn => "checked_in",
            Outcome::AutoCheckedIn => "auto_checked_in",
            Outcome::CheckedInNo => "checked_in_no",
            Outcome::Paused => "paused",
            Outcome::BreakTaken => "break_taken",
            Outcome::Resumed => "resumed",
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
    /// "Yes, still on it" at a drift check-in (§6.5). Same effect as `Ack`, kept
    /// separate because the box asks a question, not for a start.
    CheckInYes(UnixTime),
    /// "No" at a drift check-in (§6.5) → bring up the task list.
    CheckInNo(UnixTime),
    /// Tray Pause for this many seconds (§6.6), from any state.
    PauseFor(UnixTime, i64),
    /// Take-a-break for this many seconds, from the §6.5 check-in or its list.
    BreakFor(UnixTime, i64),
    /// End a pause/break early (tray Resume).
    Resume(UnixTime),
    /// A §6.4 picker (or, come Tier-C, the §6.5 list) row was clicked, carrying
    /// that row's `tasks` rowid — unlike `Ack`, the picked task matters, because
    /// classification routes tools to *its* lists.
    PickTask(UnixTime, i64),
    /// One tool's routing choice at the classification screen. Core stays in
    /// `Classifying` on each of these — the persistence is an svc side effect —
    /// and leaves on [`Event::ClassifyDone`].
    Classify { at: UnixTime, app_name: String, choice: ClassifyChoice },
    /// The classification screen finished (every row routed, or dismissed).
    ClassifyDone(UnixTime),
}

/// Where a classified tool goes (Tier-B P2): the task's tool list, the global
/// not-tool list, or a task-scoped ignore.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassifyChoice {
    Tool,
    NotTool,
    Ignore,
}

impl Event {
    fn now(&self) -> UnixTime {
        match self {
            Event::EdgeTimer(t)
            | Event::HotkeyToggle(t)
            | Event::RulesReloaded(t)
            | Event::Ack(t)
            | Event::Snooze(t)
            | Event::Skip(t)
            | Event::CheckInYes(t)
            | Event::CheckInNo(t)
            | Event::PauseFor(t, _)
            | Event::BreakFor(t, _)
            | Event::Resume(t)
            | Event::PickTask(t, _)
            | Event::Classify { at: t, .. }
            | Event::ClassifyDone(t) => *t,
        }
    }
}

/// Which answer buttons a prompt carries. Data, not UI logic — the svc paints
/// exactly this set and maps each to the [`Event`] named here, so "what can the
/// user say to this box" stays a property of the state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Buttons {
    /// A task window's prompt: Start→`Ack`, Snooze→`Snooze`, Skip→`Skip`. Also
    /// the periodic check-in, whose Start/Skip mean "still on it"/"leave me".
    StartSnoozeSkip,
    /// A §6.5 drift check-in: Yes→`CheckInYes`, No→`CheckInNo`, Break→`BreakFor`.
    YesNoBreak,
    /// §6.4 on-task check-in: pick a task from the due-window list (or "New
    /// task…"). Resolution routes into the classification screen (Tier-B P2),
    /// not a binary answer. P1 stub: the svc may render this by reusing the
    /// §6.5 list rows in `ctx.task_rows`; the real picker is P2/P3 work.
    TaskList,
}

/// Effects are data; nudge-svc executes them. Every create effect has a paired
/// destroy effect emitted on the exiting transition (leak discipline).
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Show (or update) the prompt overlay at the given visibility level and
    /// notification mode (OffTask strong/escalating vs. OnTask peripheral).
    ShowPrompt { text: String, level: Level, mode: Mode, buttons: Buttons },
    /// Hide the prompt overlay.
    HidePrompt,
    /// Show the §6.5 task list. `rows` are [`task_window::display_list`] output
    /// verbatim — already selected, sorted, capped and styled — so the svc's
    /// render is a paint loop with no logic in it.
    ///
    /// [`task_window::display_list`]: crate::task_window::display_list
    ShowTaskList { rows: Vec<Row> },
    /// Tear down the task list window.
    HideTaskList,
    /// Show the Tier-B P2 classification screen for `tools` (the svc's
    /// since-last-check-in accumulator, snapshotted into `ctx.classify_tools`),
    /// routing each to `task_id`'s lists. Core copies; the svc renders and
    /// persists.
    ShowClassify { tools: Vec<String>, task_id: i64 },
    /// Tear down the classification screen.
    HideClassify,
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
    /// Take-a-break duration offered on the §6.5 No path, in seconds.
    pub break_secs: i64,
    /// The §6.5 task list, precomputed by the caller via
    /// [`task_window::display_list`] — core neither selects nor sorts nor styles
    /// it, matching the "one owner" rule (§6.9). The svc fills this only while a
    /// drift check-in could be answered No; empty everywhere else, so the common
    /// transition pays nothing for it.
    ///
    /// [`task_window::display_list`]: crate::task_window::display_list
    pub task_rows: Vec<Row>,
    /// Was the foreground app one of the live task's tools at a sample edge? The
    /// svc probes AW and does the set comparison; core only branches on the
    /// answer. `true` everywhere else (and when AW is down), so a missing signal
    /// never manufactures an off-task run.
    pub foreground_on_task: bool,
    /// §6.4: was the foreground app in the tool list of ANY task in the dynamic
    /// deadline window (not just the live task)? The svc does the union + set
    /// compare at a due on-task tick; core only branches. `true` everywhere
    /// else (and when AW is down / nothing configured), so a missing signal
    /// never manufactures an on-task nag.
    pub any_task_on_task: bool,
    /// §6.4 on-task check-in floor cadence in seconds; `None` disables it (no
    /// `ontask_at` is ever armed). From `[escalation] ontask_checkin_secs`.
    pub ontask_secs: Option<i64>,
    /// Tools seen since the last check-in (the svc's accumulator, PLAN-step3 §1
    /// option A), snapshotted here only while a check-in that can enter
    /// classification is on screen; empty everywhere else. Empty at the moment
    /// of resolution ⇒ nothing to classify ⇒ the check-in resolves straight to
    /// `Started` as it did pre-P2.
    pub classify_tools: Vec<String>,
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

    // Pause is reachable from anywhere and outranks everything, including the
    // window-close collapse below: a paused svc must not be woken by the
    // schedule, so its edge is armed bare and the schedule is recomputed on
    // resume. Same for the quiet states it leads to.
    if let Event::PauseFor(_, secs) = event {
        return enter_quiet(state, now, now + secs, Quiet::Pause);
    }
    match state {
        State::Paused { resume_at, was_started } => {
            return quiet_tick(state, resume_at, was_started, Quiet::Pause, event, now, ctx)
        }
        State::Break { resume_at } => {
            return quiet_tick(state, resume_at, true, Quiet::Break, event, now, ctx)
        }
        _ => {}
    }

    // Manual toggle is orthogonal to the window lifecycle.
    if let Event::HotkeyToggle(_) = event {
        return hotkey_toggle(state, now, ctx);
    }

    // Window closed (or none defined): everything collapses to Idle.
    if !ctx.in_window {
        if !matches!(state, State::Idle) {
            hide_visible(state, &mut fx);
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
            let ontask_at = ctx.ontask_secs.map(|s| now + s);
            fx.push(Effect::HidePrompt);
            fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
            fx.push(Effect::LogOutcome {
                outcome: Outcome::Started,
                at: now,
            });
            arm_started(checkin_at, sample_at, ontask_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at,
                    sample_at,
                    off_task_since: None,
                    ontask_at,
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
                    ontask_at: None,
                },
                fx,
            )
        }

        // A due edge (check-in, on-task tick, or sample) fired, or we're just
        // re-arming. Three runtime edges now live in `Started`; all are merged
        // into the single timer by `arm_started`, and whichever is actually due
        // drives the branch, read top-to-bottom checkin → ontask → sample.
        (State::Started { checkin_at, sample_at, off_task_since, ontask_at }, Event::EdgeTimer(_))
        | (State::Started { checkin_at, sample_at, off_task_since, ontask_at }, Event::RulesReloaded(_)) => {
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
                        // A sibling edge may have come due on this same wake;
                        // advance it rather than re-arming a time already past.
                        let sample_at = bump_if_due(sample_at, now, ctx.sample_secs);
                        let ontask_at = bump_if_due(ontask_at, now, ctx.ontask_secs);
                        arm_started(None, sample_at, ontask_at, ctx, &mut fx);
                        return (
                            State::Started {
                                checkin_at: None,
                                sample_at,
                                off_task_since,
                                ontask_at,
                            },
                            fx,
                        );
                    }
                    show_checkin(CheckInKind::Periodic, ctx, &mut fx);
                    fx.push(Effect::LogEdge { entered: "checkin", at: now, mode: Some(Mode::OffTask) });
                    arm_schedule(ctx, &mut fx);
                    return (State::CheckIn { shown_at: now, kind: CheckInKind::Periodic }, fx);
                }
            }

            // --- on-task check-in tick (§6.4): off *every* due task's tools? ---
            if let Some(ot) = ontask_at {
                if now >= ot {
                    if !ctx.any_task_on_task {
                        show_checkin(CheckInKind::OnTask, ctx, &mut fx);
                        fx.push(Effect::LogEdge { entered: "checkin", at: now, mode: Some(Mode::OffTask) });
                        arm_schedule(ctx, &mut fx);
                        return (State::CheckIn { shown_at: now, kind: CheckInKind::OnTask }, fx);
                    }
                    // On some task's tools at the tick → the floor says stay
                    // quiet; re-arm the next tick, bumping a co-due sample edge.
                    let next_ot = ctx.ontask_secs.map(|s| now + s);
                    let sample_at = bump_if_due(sample_at, now, ctx.sample_secs);
                    arm_started(checkin_at, sample_at, next_ot, ctx, &mut fx);
                    return (
                        State::Started {
                            checkin_at,
                            sample_at,
                            off_task_since,
                            ontask_at: next_ot,
                        },
                        fx,
                    );
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
                        arm_started(checkin_at, next_sample, ontask_at, ctx, &mut fx);
                        return (
                            State::Started {
                                checkin_at,
                                sample_at: next_sample,
                                off_task_since: None,
                                ontask_at,
                            },
                            fx,
                        );
                    }
                    // Off-task: the run starts at the first off-task sample and is
                    // measured from there, so the threshold is wall-clock drift,
                    // not a sample count.
                    let since = off_task_since.unwrap_or(now);
                    if now - since >= ctx.off_task_secs {
                        show_checkin(CheckInKind::OffTask, ctx, &mut fx);
                        fx.push(Effect::LogEdge { entered: "checkin", at: now, mode: Some(Mode::OffTask) });
                        arm_schedule(ctx, &mut fx);
                        return (State::CheckIn { shown_at: now, kind: CheckInKind::OffTask }, fx);
                    }
                    arm_started(checkin_at, next_sample, ontask_at, ctx, &mut fx);
                    return (
                        State::Started {
                            checkin_at,
                            sample_at: next_sample,
                            off_task_since: Some(since),
                            ontask_at,
                        },
                        fx,
                    );
                }
            }

            // --- nothing due: re-arm all edges unchanged ---
            arm_started(checkin_at, sample_at, ontask_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at,
                    sample_at,
                    off_task_since,
                    ontask_at,
                },
                fx,
            )
        }

        // Affirming a check-in with tools accumulated since the last one routes
        // through the classification screen (Tier-B P2) instead of straight to
        // Started: §6.5 Yes classifies against the open window's task, the §6.4
        // picker against the picked row's. With nothing accumulated (or no task
        // to route to) the guard fails and the plain resolution below applies,
        // exactly as pre-P2. Periodic's Ack never lands here — no tool set.
        (State::CheckIn { kind: CheckInKind::OffTask, .. }, Event::CheckInYes(_))
            if !ctx.classify_tools.is_empty() && ctx.window_task_id.is_some() =>
        {
            let task_id = ctx.window_task_id.expect("guarded");
            enter_classifying(state, task_id, now, ctx, fx)
        }
        (State::CheckIn { kind: CheckInKind::OnTask, .. }, Event::PickTask(_, id))
            if !ctx.classify_tools.is_empty() =>
        {
            enter_classifying(state, *id, now, ctx, fx)
        }

        // Yes ("still on it") returns to Started and resumes the sampling spine
        // with a fresh off-task run; Skip ("leave me alone") stops it, matching
        // Skip's dismiss-for-good meaning at the prompt. Ack is Yes under the
        // periodic check-in's Start/Skip button set. A row pick with no tools to
        // classify resolves the same way (the id becomes meaningful in Tier-C).
        (State::CheckIn { .. }, Event::Ack(_))
        | (State::CheckIn { .. }, Event::CheckInYes(_))
        | (State::CheckIn { .. }, Event::Skip(_))
        | (State::CheckIn { .. }, Event::PickTask(_, _))
        // Picking a task off the §6.5 list (or dismissing it) resolves the same
        // way: the list is how we asked "so what *are* you doing?", and either
        // answer ends the question.
        | (State::Choosing { .. }, Event::Ack(_))
        | (State::Choosing { .. }, Event::PickTask(_, _))
        | (State::Choosing { .. }, Event::Skip(_)) => {
            let yes = matches!(
                event,
                Event::Ack(_) | Event::CheckInYes(_) | Event::PickTask(_, _)
            );
            let outcome = if yes { Outcome::CheckedIn } else { Outcome::Skipped };
            let sample_at = if yes { ctx.sample_secs.map(|s| now + s) } else { None };
            let ontask_at = if yes { ctx.ontask_secs.map(|s| now + s) } else { None };
            hide_visible(state, &mut fx);
            fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
            fx.push(Effect::LogOutcome { outcome, at: now });
            arm_started(None, sample_at, ontask_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at: None,
                    sample_at,
                    off_task_since: None,
                    ontask_at,
                },
                fx,
            )
        }

        // No: swap the question for the list of what's actually due (§6.5). The
        // rows arrive precomputed in `ctx` — core picks nothing. Still a
        // check-in-family state, so an ignored list dies at the next schedule edge
        // rather than sitting on screen forever.
        (State::CheckIn { .. }, Event::CheckInNo(_)) => {
            fx.push(Effect::HidePrompt);
            fx.push(Effect::ShowTaskList { rows: ctx.task_rows.clone() });
            fx.push(Effect::LogEdge { entered: "choosing", at: now, mode: None });
            fx.push(Effect::LogOutcome { outcome: Outcome::CheckedInNo, at: now });
            arm_schedule(ctx, &mut fx);
            (State::Choosing { shown_at: now }, fx)
        }

        // Take-a-break, from the check-in box, the list it opened, or the
        // classification screen.
        (State::CheckIn { .. }, Event::BreakFor(_, secs))
        | (State::Choosing { .. }, Event::BreakFor(_, secs))
        | (State::Classifying { .. }, Event::BreakFor(_, secs)) => {
            enter_quiet(state, now, now + secs, Quiet::Break)
        }

        // One tool routed at the classification screen. The persistence is an
        // svc side effect at the event seam — core holds no tool list, so there
        // is nothing to update here; just keep the screen up and stay armed.
        (State::Classifying { .. }, Event::Classify { .. }) => {
            arm_schedule(ctx, &mut fx);
            (state, fx)
        }

        // Classification finished (all rows routed) or dismissed: the check-in
        // it grew out of is answered, so land in Started with the sampling
        // spine and the §6.4 tick freshly armed. The svc clears its accumulator
        // on this resolution.
        (State::Classifying { .. }, Event::ClassifyDone(_))
        | (State::Classifying { .. }, Event::Skip(_)) => {
            let sample_at = ctx.sample_secs.map(|s| now + s);
            let ontask_at = ctx.ontask_secs.map(|s| now + s);
            fx.push(Effect::HideClassify);
            fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
            fx.push(Effect::LogOutcome { outcome: Outcome::CheckedIn, at: now });
            arm_started(None, sample_at, ontask_at, ctx, &mut fx);
            (
                State::Started {
                    checkin_at: None,
                    sample_at,
                    off_task_since: None,
                    ontask_at,
                },
                fx,
            )
        }

        // Reload while classifying: re-emit so a restart keeps the screen up.
        (State::Classifying { task_id, shown_at }, Event::RulesReloaded(_)) => {
            fx.push(Effect::ShowClassify { tools: ctx.classify_tools.clone(), task_id });
            arm_schedule(ctx, &mut fx);
            (State::Classifying { task_id, shown_at }, fx)
        }

        // Reload while a check-in or its list is showing: re-emit so a restart
        // keeps it on screen.
        (State::CheckIn { shown_at, kind }, Event::RulesReloaded(_)) => {
            show_checkin(kind, ctx, &mut fx);
            arm_schedule(ctx, &mut fx);
            (State::CheckIn { shown_at, kind }, fx)
        }
        (State::Choosing { shown_at }, Event::RulesReloaded(_)) => {
            fx.push(Effect::ShowTaskList { rows: ctx.task_rows.clone() });
            arm_schedule(ctx, &mut fx);
            (State::Choosing { shown_at }, fx)
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
            buttons: Buttons::StartSnoozeSkip,
        });
        arm_schedule(ctx, fx);
        return;
    }
    let step = evaluate(&ctx.ladder, shown_at, now);
    fx.push(Effect::ShowPrompt {
        text: ctx.window_text.clone(),
        level: step.level,
        mode,
        buttons: Buttons::StartSnoozeSkip,
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
            hide_visible(state, &mut fx);
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

/// Swap an affirmed check-in for the classification screen (Tier-B P2): tear
/// down whatever the check-in had up, show the tool router for `task_id`, and
/// log the family transition. The check-in's outcome is logged here (the user
/// answered it); `ClassifyDone` later logs only the started edge.
fn enter_classifying(
    state: State,
    task_id: i64,
    now: UnixTime,
    ctx: &ScheduleCtx,
    mut fx: Vec<Effect>,
) -> (State, Vec<Effect>) {
    hide_visible(state, &mut fx);
    fx.push(Effect::ShowClassify { tools: ctx.classify_tools.clone(), task_id });
    fx.push(Effect::LogEdge { entered: "classifying", at: now, mode: None });
    fx.push(Effect::LogOutcome { outcome: Outcome::CheckedIn, at: now });
    arm_schedule(ctx, &mut fx);
    (State::Classifying { task_id, shown_at: now }, fx)
}

/// Put a check-in on screen. Both kinds render as an attention-getting `OffTask`
/// L0 prompt regardless of the window's own mode — a check-in is a discrete
/// question, not a background cue — and differ only in what the user can answer.
fn show_checkin(kind: CheckInKind, ctx: &ScheduleCtx, fx: &mut Vec<Effect>) {
    fx.push(Effect::ShowPrompt {
        text: checkin_text(ctx),
        level: Level::L0,
        mode: Mode::OffTask,
        buttons: match kind {
            CheckInKind::Periodic => Buttons::StartSnoozeSkip,
            CheckInKind::OffTask => Buttons::YesNoBreak,
            CheckInKind::OnTask => Buttons::TaskList,
        },
    });
    // The §6.4 picker IS the task list: rows arrive precomputed (the svc fills
    // `task_rows` at a due on-task tick), and a row click sends `PickTask` with
    // that row's id, which is what routes into classification.
    if kind == CheckInKind::OnTask {
        fx.push(Effect::ShowTaskList { rows: ctx.task_rows.clone() });
    }
}

/// Tear down whatever `state` has on screen: the prompt strip always (dropping a
/// prompt that isn't up is a no-op in the svc), the §6.5 list only when it is
/// actually showing. Keeps paired teardown in one place rather than asking every
/// exiting transition to remember which windows it owns.
fn hide_visible(state: State, fx: &mut Vec<Effect>) {
    fx.push(Effect::HidePrompt);
    if matches!(
        state,
        State::Choosing { .. } | State::CheckIn { kind: CheckInKind::OnTask, .. }
    ) {
        fx.push(Effect::HideTaskList);
    }
    if matches!(state, State::Classifying { .. }) {
        fx.push(Effect::HideClassify);
    }
}

/// The two silenced states, which differ only in bookkeeping.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Quiet {
    Pause,
    Break,
}

impl Quiet {
    fn edge(self) -> EdgeKind {
        match self {
            Quiet::Pause => EdgeKind::PauseExpiry,
            Quiet::Break => EdgeKind::BreakExpiry,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Quiet::Pause => "paused",
            Quiet::Break => "break",
        }
    }
    fn outcome(self) -> Outcome {
        match self {
            Quiet::Pause => Outcome::Paused,
            Quiet::Break => Outcome::BreakTaken,
        }
    }
}

/// Go quiet until `resume_at`: everything off screen, and **one bare edge** —
/// `ctx.next_edge` is deliberately not merged, which is what makes a pause a
/// pause. The schedule is recomputed from scratch on resume, so suppressing it
/// here loses nothing.
fn enter_quiet(state: State, now: UnixTime, resume_at: UnixTime, q: Quiet) -> (State, Vec<Effect>) {
    let mut fx = Vec::new();
    hide_visible(state, &mut fx);
    fx.push(Effect::LogEdge { entered: q.label(), at: now, mode: None });
    fx.push(Effect::LogOutcome { outcome: q.outcome(), at: now });
    fx.push(Effect::ArmEdgeTimer { at: resume_at, kind: q.edge() });
    let next = match q {
        // A break is only ever reached from a live task, so it always resumes to
        // one. A pause can come from anywhere — remember whether there was a task
        // in flight, so resuming doesn't nag the user to start what they were
        // already doing.
        Quiet::Pause => State::Paused { resume_at, was_started: was_live(state) },
        Quiet::Break => State::Break { resume_at },
    };
    (next, fx)
}

/// Was a task in flight in `state`? A showing check-in (and its list) counts: the
/// user started the task, we're only asking about it.
fn was_live(state: State) -> bool {
    matches!(
        state,
        State::Started { .. }
            | State::CheckIn { .. }
            | State::Choosing { .. }
            | State::Classifying { .. }
    )
}

/// A wake while paused/on-break. Only expiry or an explicit `Resume` ends it;
/// every other event re-arms the same bare edge and changes nothing, so a hotkey
/// press, a reload, or a stray click cannot break the silence.
fn quiet_tick(
    state: State,
    resume_at: UnixTime,
    was_started: bool,
    q: Quiet,
    event: &Event,
    now: UnixTime,
    ctx: &ScheduleCtx,
) -> (State, Vec<Effect>) {
    let over = matches!(event, Event::Resume(_))
        || (matches!(event, Event::EdgeTimer(_)) && now >= resume_at);
    if over {
        return resume(was_started, now, ctx);
    }
    let fx = vec![Effect::ArmEdgeTimer { at: resume_at, kind: q.edge() }];
    (state, fx)
}

/// Come back from a pause/break: re-derive from the schedule as it stands *now*
/// (it was never armed while quiet, so there is nothing stale to honour). A task
/// that was live resumes live, with its runtime edges freshly armed from `now`.
fn resume(was_started: bool, now: UnixTime, ctx: &ScheduleCtx) -> (State, Vec<Effect>) {
    let mut fx = vec![Effect::LogOutcome { outcome: Outcome::Resumed, at: now }];
    if !ctx.in_window {
        fx.push(Effect::LogEdge { entered: "idle", at: now, mode: None });
        arm_schedule(ctx, &mut fx);
        return (State::Idle, fx);
    }
    if was_started {
        let checkin_at = ctx.checkin_after_secs.map(|s| now + s);
        let sample_at = ctx.sample_secs.map(|s| now + s);
        let ontask_at = ctx.ontask_secs.map(|s| now + s);
        fx.push(Effect::LogEdge { entered: "started", at: now, mode: None });
        arm_started(checkin_at, sample_at, ontask_at, ctx, &mut fx);
        return (
            State::Started { checkin_at, sample_at, off_task_since: None, ontask_at },
            fx,
        );
    }
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

/// Arm just the schedule edge (no competing runtime edge).
fn arm_schedule(ctx: &ScheduleCtx, fx: &mut Vec<Effect>) {
    arm_merged(None, ctx, fx);
}

/// Arm the single timer for a `Started` state, whose three runtime edges
/// (check-in, sample, §6.4 on-task tick) collapse to their earliest before
/// being merged with the schedule edge. `Started` is the only state with
/// multiple runtime candidates; funnelling them through here keeps the
/// one-armed-timer invariant a property of the code rather than of every
/// caller remembering it.
fn arm_started(
    checkin_at: Option<UnixTime>,
    sample_at: Option<UnixTime>,
    ontask_at: Option<UnixTime>,
    ctx: &ScheduleCtx,
    fx: &mut Vec<Effect>,
) {
    let runtime = earliest(
        earliest(
            checkin_at.map(edge(EdgeKind::CheckIn)),
            sample_at.map(edge(EdgeKind::Sample)),
        ),
        ontask_at.map(edge(EdgeKind::OnTaskCheckIn)),
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
    use crate::task_window::StyleClass;

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
            break_secs: 600,
            task_rows: Vec::new(),
            foreground_on_task: true,
            any_task_on_task: true,
            ontask_secs: None,
            classify_tools: Vec::new(),
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
            ontask_at: None,
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
        assert!(fx.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L0, mode: Mode::OffTask, buttons: Buttons::StartSnoozeSkip }));
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
        assert!(fx.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L1, mode: Mode::OffTask, buttons: Buttons::StartSnoozeSkip }));
        assert!(!fx.contains(&Effect::PlaySound));
        // One repeat past L2 (2200) → re-alert with sound.
        let (_, fx2) = next(s, &Event::EdgeTimer(2200), &ctx(true, window_end(9999)));
        assert!(fx2.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L2, mode: Mode::OffTask, buttons: Buttons::StartSnoozeSkip }));
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
            mode: Mode::OnTask, buttons: Buttons::StartSnoozeSkip
        }));
        assert_eq!(armed(&fx), Some((9999, EdgeKind::WindowEnd)));
        // A later tick past the OffTask L2 boundary still shows L0, no sound.
        let (_, fx2) = next(s, &Event::EdgeTimer(2200), &c);
        assert!(fx2.contains(&Effect::ShowPrompt {
            text: "focus".into(),
            level: Level::L0,
            mode: Mode::OnTask, buttons: Buttons::StartSnoozeSkip
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
        assert!(matches!(s, State::CheckIn { shown_at: 3000, .. }));
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
            assert!(matches!(s, State::CheckIn { shown_at: 3000, .. }), "presence {p:?}");
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
        assert!(fx3.contains(&Effect::ShowPrompt { text: "focus".into(), level: Level::L0, mode: Mode::OffTask, buttons: Buttons::StartSnoozeSkip }));
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
            State::CheckIn { shown_at: 3000, kind: CheckInKind::Periodic },
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
                off_task_since: None, ontask_at: None
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
            ontask_at: None,
        };
        let (s, fx) = next(st, &Event::EdgeTimer(1500), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(1800),
                off_task_since: None, ontask_at: None
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
            ontask_at: None,
        };

        // First off-task sample: run opens at 1500, still under threshold.
        let (s, fx) = next(st, &Event::EdgeTimer(1500), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(1800),
                off_task_since: Some(1500), ontask_at: None
            }
        );
        assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));

        // Second sample at 1800: the run is exactly 300s → threshold crossed.
        let (s2, fx2) = next(s, &Event::EdgeTimer(1800), &c);
        assert!(matches!(s2, State::CheckIn { shown_at: 1800, .. }));
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
            ontask_at: None,
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
            ontask_at: None,
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
        let (_, fx) = next(State::CheckIn { shown_at: 3000, kind: CheckInKind::Periodic }, &Event::EdgeTimer(3100), &c);
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
            ontask_at: None,
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
            ontask_at: None,
        };
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(3300), // bumped, not left at 3000
                off_task_since: None, ontask_at: None
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
        let (s, fx) = next(State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask }, &Event::Ack(1900), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(2200),
                off_task_since: None, ontask_at: None
            }
        );
        assert_eq!(armed(&fx), Some((2200, EdgeKind::Sample)));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 1900 }));

        let (s2, fx2) = next(State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask }, &Event::Skip(1900), &c);
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
            ontask_at: None,
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

    // --- §6.5 drift check-in Yes/No + task list, §6.6 Pause/Break ---

    fn row(task_id: i64, title: &str) -> Row {
        Row {
            task_id,
            title: title.into(),
            deadline: Some(9_000),
            logged: 0,
            estimate: None,
            style: StyleClass::NotStarted,
        }
    }

    /// Sampling context carrying a §6.5 list, as the svc supplies at a live
    /// drift check-in.
    fn listed_ctx(edge: Option<Edge>) -> ScheduleCtx {
        let mut c = sampling_ctx(edge);
        c.task_rows = vec![row(1, "thesis"), row(2, "email")];
        c
    }

    fn shown(fx: &[Effect]) -> Option<&Effect> {
        fx.iter().find(|e| matches!(e, Effect::ShowPrompt { .. }))
    }

    /// Drive an off-task run over the threshold, landing on the drift check-in.
    fn drifted(c: &ScheduleCtx) -> State {
        let st = State::Started {
            checkin_at: None,
            sample_at: Some(1500),
            off_task_since: Some(1500),
            ontask_at: None,
        };
        let (s, _) = next(st, &Event::EdgeTimer(1800), c);
        s
    }

    // The drift check-in is a question, not a start: it carries its kind and the
    // Yes/No/Break buttons, where the periodic one keeps Start/Snooze/Skip.
    #[test]
    fn drift_checkin_asks_yes_no_periodic_does_not() {
        let mut c = listed_ctx(window_end(9999));
        c.foreground_on_task = false;
        let s = drifted(&c);
        assert!(matches!(s, State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask }));
        let (_, fx) = next(
            State::Started { checkin_at: Some(1500), sample_at: None, off_task_since: None, ontask_at: None },
            &Event::EdgeTimer(1500),
            &c,
        );
        assert!(matches!(
            shown(&fx),
            Some(Effect::ShowPrompt { buttons: Buttons::StartSnoozeSkip, .. })
        ));
    }

    // Yes: back to work, sampling spins up again with a clean off-task run.
    #[test]
    fn checkin_yes_resumes_the_task_and_sampling() {
        let c = listed_ctx(window_end(9999));
        let st = State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask };
        let (s, fx) = next(st, &Event::CheckInYes(1900), &c);
        assert_eq!(
            s,
            State::Started { checkin_at: None, sample_at: Some(2200), off_task_since: None, ontask_at: None }
        );
        assert!(fx.contains(&Effect::HidePrompt));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 1900 }));
        assert_eq!(armed(&fx), Some((2200, EdgeKind::Sample)));
    }

    // No: the question comes down, the list goes up — carrying exactly the rows
    // the caller computed, in order, with nothing added or reordered by core.
    #[test]
    fn checkin_no_shows_the_task_list_verbatim() {
        let c = listed_ctx(window_end(9999));
        let st = State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask };
        let (s, fx) = next(st, &Event::CheckInNo(1900), &c);
        assert!(matches!(s, State::Choosing { shown_at: 1900 }));
        assert!(fx.contains(&Effect::HidePrompt));
        assert!(fx.contains(&Effect::ShowTaskList { rows: c.task_rows.clone() }));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::CheckedInNo, at: 1900 }));
        // No sample edge competes with a list the user is reading.
        assert_eq!(armed(&fx), Some((9999, EdgeKind::WindowEnd)));
    }

    // An ignored list dies at the next schedule edge, exactly as an ignored
    // check-in does — and takes its window with it (paired teardown).
    #[test]
    fn ignored_task_list_dismisses_at_the_schedule_edge() {
        let out = ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart }));
        let (s, fx) = next(State::Choosing { shown_at: 1900 }, &Event::EdgeTimer(5000), &out);
        assert_eq!(s, State::Idle);
        assert!(fx.contains(&Effect::HideTaskList));
        assert_eq!(armed(&fx), Some((90_000, EdgeKind::TaskStart)));
    }

    // Picking a task off the list (Start) resolves the question and resumes
    // supervision; dismissing it (Skip) resolves it and stops sampling.
    #[test]
    fn choosing_resolves_on_either_answer() {
        let c = listed_ctx(window_end(9999));
        let (s, fx) = next(State::Choosing { shown_at: 1900 }, &Event::Ack(2000), &c);
        assert_eq!(
            s,
            State::Started { checkin_at: None, sample_at: Some(2300), off_task_since: None, ontask_at: None }
        );
        assert!(fx.contains(&Effect::HideTaskList));

        let (s2, fx2) = next(State::Choosing { shown_at: 1900 }, &Event::Skip(2000), &c);
        assert_eq!(s2, started(None));
        assert!(fx2.contains(&Effect::HideTaskList));
        assert!(fx2.contains(&Effect::LogOutcome { outcome: Outcome::Skipped, at: 2000 }));
    }

    // Take-a-break from the list: everything off screen, one break edge, and the
    // task resumes when it expires.
    #[test]
    fn break_from_the_list_silences_then_resumes_the_task() {
        let c = listed_ctx(window_end(99_999));
        let (s, fx) = next(State::Choosing { shown_at: 1900 }, &Event::BreakFor(2000, 600), &c);
        assert_eq!(s, State::Break { resume_at: 2600 });
        assert!(fx.contains(&Effect::HideTaskList));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::BreakTaken, at: 2000 }));
        assert_eq!(armed(&fx), Some((2600, EdgeKind::BreakExpiry)));

        let (s2, fx2) = next(s, &Event::EdgeTimer(2600), &c);
        assert_eq!(
            s2,
            State::Started { checkin_at: None, sample_at: Some(2900), off_task_since: None, ontask_at: None }
        );
        assert!(fx2.contains(&Effect::LogOutcome { outcome: Outcome::Resumed, at: 2600 }));
    }

    // Pause is reachable from every state, and each time it silences everything
    // down to a single expiry edge — the schedule edge included. This is the
    // §6.6 invariant: nothing fires mid-pause.
    #[test]
    fn pause_from_any_state_arms_only_its_expiry() {
        // A window end at 2100 would otherwise beat the 2600 expiry; it must not
        // be armed — that is the difference between a pause and a mute.
        let c = listed_ctx(window_end(2100));
        for st in [
            State::Idle,
            prompting(),
            State::Started { checkin_at: Some(2050), sample_at: Some(2050), off_task_since: None, ontask_at: None },
            State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask },
            State::Choosing { shown_at: 1900 },
        ] {
            let (s, fx) = next(st, &Event::PauseFor(2000, 600), &c);
            assert!(matches!(s, State::Paused { resume_at: 2600, .. }), "from {}", st.label());
            assert!(fx.contains(&Effect::HidePrompt), "from {}", st.label());
            assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::Paused, at: 2000 }));
            assert_eq!(armed(&fx), Some((2600, EdgeKind::PauseExpiry)), "from {}", st.label());
            assert_eq!(
                fx.iter().filter(|e| matches!(e, Effect::ArmEdgeTimer { .. })).count(),
                1,
                "from {}",
                st.label()
            );
        }
    }

    // Nothing gets through a pause: no wake re-shows a prompt, and no event other
    // than expiry or Resume ends it.
    #[test]
    fn paused_stays_silent_until_expiry_or_resume() {
        let c = listed_ctx(window_end(2100));
        let paused = State::Paused { resume_at: 2600, was_started: true };
        for ev in [
            Event::EdgeTimer(2500),
            Event::HotkeyToggle(2500),
            Event::RulesReloaded(2500),
            Event::Ack(2500),
            Event::CheckInNo(2500),
        ] {
            let (s, fx) = next(paused, &ev, &c);
            assert_eq!(s, paused, "event {ev:?}");
            assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. } | Effect::ShowTaskList { .. })));
            assert_eq!(armed(&fx), Some((2600, EdgeKind::PauseExpiry)), "event {ev:?}");
        }
        // Even a window close can't wake it: the schedule was never armed, and a
        // pause outlives the window it started in.
        let (s, _) = next(paused, &Event::EdgeTimer(2500), &ctx(false, None));
        assert_eq!(s, paused);
    }

    // Resuming re-derives from the schedule as it stands now. A task that was
    // live comes back live; a pause taken before starting returns to the prompt;
    // a window that closed meanwhile lands in Idle.
    #[test]
    fn resume_recomputes_from_the_current_schedule() {
        let c = listed_ctx(window_end(9999));

        // Was started → Started, edges freshly armed from the resume instant.
        let (s, fx) = next(
            State::Paused { resume_at: 2600, was_started: true },
            &Event::EdgeTimer(2600),
            &c,
        );
        assert_eq!(
            s,
            State::Started { checkin_at: None, sample_at: Some(2900), off_task_since: None, ontask_at: None }
        );
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::Resumed, at: 2600 }));
        assert_eq!(armed(&fx), Some((2900, EdgeKind::Sample)));

        // Was only prompting → back to the prompt, ladder restarted from now.
        let (s2, fx2) = next(
            State::Paused { resume_at: 2600, was_started: false },
            &Event::Resume(2600),
            &c,
        );
        assert!(matches!(s2, State::Prompting { shown_at: 2600, .. }));
        assert!(fx2.iter().any(|e| matches!(e, Effect::ShowPrompt { level: Level::L0, .. })));

        // Window closed during the pause → Idle, schedule edge armed.
        let out = ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart }));
        let (s3, fx3) = next(
            State::Paused { resume_at: 2600, was_started: true },
            &Event::EdgeTimer(2600),
            &out,
        );
        assert_eq!(s3, State::Idle);
        assert_eq!(armed(&fx3), Some((90_000, EdgeKind::TaskStart)));
    }

    // --- §6.4 on-task check-in (Tier B P1) ---

    /// Sampling context with the on-task cadence live at 30 min.
    fn ontask_ctx(edge: Option<Edge>) -> ScheduleCtx {
        let mut c = sampling_ctx(edge);
        c.ontask_secs = Some(1800);
        c
    }

    // Ack arms the on-task tick one cadence out; disabled config never arms it.
    #[test]
    fn ack_arms_ontask_tick_when_enabled() {
        let (s, _) = next(prompting(), &Event::Ack(1200), &ontask_ctx(window_end(99_999)));
        assert!(matches!(s, State::Started { ontask_at: Some(3000), .. }));

        // ontask_secs = None → no tick, ever.
        let (s2, _) = next(prompting(), &Event::Ack(1200), &sampling_ctx(window_end(99_999)));
        assert!(matches!(s2, State::Started { ontask_at: None, .. }));
    }

    // Tick due while off every due task's tools → the §6.4 check-in, with the
    // task-list button set, and no sample edge competing with it.
    #[test]
    fn ontask_tick_prompts_when_off_every_tasks_tools() {
        let mut c = ontask_ctx(window_end(99_999));
        c.any_task_on_task = false;
        let st = State::Started {
            checkin_at: None,
            sample_at: None,
            off_task_since: None,
            ontask_at: Some(3000),
        };
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert!(matches!(s, State::CheckIn { shown_at: 3000, kind: CheckInKind::OnTask }));
        assert!(matches!(
            shown(&fx),
            Some(Effect::ShowPrompt { buttons: Buttons::TaskList, .. })
        ));
        assert!(fx.contains(&Effect::LogEdge { entered: "checkin", at: 3000, mode: Some(Mode::OffTask) }));
        assert_eq!(armed(&fx), Some((99_999, EdgeKind::WindowEnd)));
    }

    // Tick due while on some task's tools → floor semantics: stay quiet, next
    // tick armed one cadence out.
    #[test]
    fn ontask_tick_is_silent_floor_when_on_any_task() {
        let c = ontask_ctx(window_end(99_999)); // any_task_on_task defaults true
        let st = State::Started {
            checkin_at: None,
            sample_at: None,
            off_task_since: None,
            ontask_at: Some(3000),
        };
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert!(matches!(s, State::Started { ontask_at: Some(4800), .. }));
        assert!(!fx.iter().any(|e| matches!(e, Effect::ShowPrompt { .. })));
        assert_eq!(armed(&fx), Some((4800, EdgeKind::OnTaskCheckIn)));
    }

    // A sample co-due with a silent on-task tick is bumped, not re-armed in the
    // past — and exactly one timer is armed.
    #[test]
    fn ontask_tick_bumps_a_co_due_sample() {
        let c = ontask_ctx(window_end(99_999));
        let st = State::Started {
            checkin_at: None,
            sample_at: Some(3000),
            off_task_since: None,
            ontask_at: Some(3000),
        };
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert_eq!(
            s,
            State::Started {
                checkin_at: None,
                sample_at: Some(3300),
                off_task_since: None,
                ontask_at: Some(4800),
            }
        );
        assert_eq!(armed(&fx), Some((3300, EdgeKind::Sample)));
        assert_eq!(
            fx.iter().filter(|e| matches!(e, Effect::ArmEdgeTimer { .. })).count(),
            1
        );
    }

    // arm_started picks the earliest of all three runtime edges vs the schedule.
    #[test]
    fn started_arms_earliest_of_three_runtime_edges() {
        let mut c = ontask_ctx(window_end(99_999));
        c.ontask_secs = Some(100); // ontask at 1300 beats sample at 1500
        let (_, fx) = next(prompting(), &Event::Ack(1200), &c);
        assert_eq!(armed(&fx), Some((1300, EdgeKind::OnTaskCheckIn)));

        // Sample sooner → sample wins.
        c.ontask_secs = Some(3600);
        let (_, fx) = next(prompting(), &Event::Ack(1200), &c);
        assert_eq!(armed(&fx), Some((1500, EdgeKind::Sample)));
    }

    // Skip means "don't nag": no on-task tick after a skipped window.
    #[test]
    fn skip_clears_the_ontask_tick() {
        let (s, _) = next(prompting(), &Event::Skip(1200), &ontask_ctx(window_end(99_999)));
        assert!(matches!(s, State::Started { ontask_at: None, .. }));
    }

    // Answering a check-in Yes re-arms the tick; resume-was_started re-arms too.
    #[test]
    fn checkin_yes_and_resume_rearm_the_ontask_tick() {
        let c = ontask_ctx(window_end(99_999));
        let st = State::CheckIn { shown_at: 1800, kind: CheckInKind::OnTask };
        let (s, _) = next(st, &Event::CheckInYes(1900), &c);
        assert!(matches!(s, State::Started { ontask_at: Some(3700), .. }));

        let (s2, _) = next(
            State::Paused { resume_at: 2600, was_started: true },
            &Event::EdgeTimer(2600),
            &c,
        );
        assert!(matches!(s2, State::Started { ontask_at: Some(4400), .. }));
    }

    // The §6.4 check-in resolves like its siblings: No brings the task list,
    // and an ignored one dies at the next schedule edge.
    #[test]
    fn ontask_checkin_no_shows_list_and_ignored_dies() {
        let c = listed_ctx(window_end(99_999));
        let st = State::CheckIn { shown_at: 3000, kind: CheckInKind::OnTask };
        let (s, fx) = next(st, &Event::CheckInNo(3100), &c);
        assert!(matches!(s, State::Choosing { shown_at: 3100 }));
        assert!(fx.contains(&Effect::ShowTaskList { rows: c.task_rows.clone() }));

        let out = ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart }));
        let (s2, fx2) = next(
            State::CheckIn { shown_at: 3000, kind: CheckInKind::OnTask },
            &Event::EdgeTimer(5000),
            &out,
        );
        assert_eq!(s2, State::Idle);
        assert!(fx2.contains(&Effect::HidePrompt));
    }

    // --- Tier-B P2: classification screen ---

    /// A live check-in with tools accumulated and a task to route them to, as
    /// the svc supplies when classification can be entered.
    fn classify_ctx(edge: Option<Edge>) -> ScheduleCtx {
        let mut c = listed_ctx(edge);
        c.ontask_secs = Some(1800);
        c.window_task_id = Some(1);
        c.classify_tools = vec!["code.exe".into(), "game.exe".into()];
        c
    }

    // The §6.4 picker is the task list: raising an OnTask check-in shows the
    // rows alongside the prompt, and both come down when a row is picked.
    #[test]
    fn ontask_checkin_shows_the_picker_rows() {
        let mut c = classify_ctx(window_end(99_999));
        c.any_task_on_task = false;
        let st = State::Started {
            checkin_at: None,
            sample_at: None,
            off_task_since: None,
            ontask_at: Some(3000),
        };
        let (s, fx) = next(st, &Event::EdgeTimer(3000), &c);
        assert!(matches!(s, State::CheckIn { kind: CheckInKind::OnTask, .. }));
        assert!(fx.contains(&Effect::ShowTaskList { rows: c.task_rows.clone() }));
    }

    // Picking a row at the §6.4 check-in enters classification for THAT task,
    // tearing the picker down and carrying the accumulator snapshot.
    #[test]
    fn ontask_pick_enters_classification_for_the_picked_task() {
        let c = classify_ctx(window_end(99_999));
        let st = State::CheckIn { shown_at: 3000, kind: CheckInKind::OnTask };
        let (s, fx) = next(st, &Event::PickTask(3100, 7), &c);
        assert_eq!(s, State::Classifying { task_id: 7, shown_at: 3100 });
        assert!(fx.contains(&Effect::HidePrompt));
        assert!(fx.contains(&Effect::HideTaskList));
        assert!(fx.contains(&Effect::ShowClassify {
            tools: c.classify_tools.clone(),
            task_id: 7
        }));
        assert!(fx.contains(&Effect::LogEdge { entered: "classifying", at: 3100, mode: None }));
        assert!(fx.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 3100 }));
    }

    // Yes at a drift check-in with tools accumulated routes through
    // classification against the open window's task — no longer straight to
    // Started (regression guard on the changed arm).
    #[test]
    fn drift_yes_with_tools_enters_classification() {
        let c = classify_ctx(window_end(9999));
        let st = State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask };
        let (s, fx) = next(st, &Event::CheckInYes(1900), &c);
        assert_eq!(s, State::Classifying { task_id: 1, shown_at: 1900 });
        assert!(fx.contains(&Effect::ShowClassify {
            tools: c.classify_tools.clone(),
            task_id: 1
        }));
    }

    // With nothing accumulated (or no task), Yes resolves as it always did; the
    // periodic check-in's Ack never classifies even with tools in hand.
    #[test]
    fn plain_resolution_when_nothing_to_classify() {
        // Empty accumulator → straight to Started.
        let mut c = classify_ctx(window_end(9999));
        c.classify_tools = Vec::new();
        let st = State::CheckIn { shown_at: 1800, kind: CheckInKind::OffTask };
        let (s, _) = next(st, &Event::CheckInYes(1900), &c);
        assert!(matches!(s, State::Started { .. }));

        // No task to route to → same.
        let mut c2 = classify_ctx(window_end(9999));
        c2.window_task_id = None;
        let (s2, _) = next(st, &Event::CheckInYes(1900), &c2);
        assert!(matches!(s2, State::Started { .. }));

        // Periodic Ack with tools in hand → still no classification.
        let c3 = classify_ctx(window_end(9999));
        let per = State::CheckIn { shown_at: 1800, kind: CheckInKind::Periodic };
        let (s3, fx3) = next(per, &Event::Ack(1900), &c3);
        assert!(matches!(s3, State::Started { .. }));
        assert!(!fx3.iter().any(|e| matches!(e, Effect::ShowClassify { .. })));

        // A row pick with no tools resolves like Start (Choosing keeps §6.5
        // behaviour with the id now riding along).
        let c4 = listed_ctx(window_end(9999));
        let (s4, fx4) = next(State::Choosing { shown_at: 1900 }, &Event::PickTask(2000, 2), &c4);
        assert!(matches!(s4, State::Started { .. }));
        assert!(fx4.contains(&Effect::HideTaskList));
        assert!(fx4.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 2000 }));
    }

    // Each routed tool keeps the screen up (persistence is the svc's); Done
    // lands in Started with the sampling spine and §6.4 tick freshly armed.
    #[test]
    fn classifying_stays_until_done_then_restarts_the_spine() {
        let c = classify_ctx(window_end(99_999));
        let st = State::Classifying { task_id: 7, shown_at: 3100 };
        let (s, fx) = next(
            st,
            &Event::Classify {
                at: 3150,
                app_name: "code.exe".into(),
                choice: ClassifyChoice::Tool,
            },
            &c,
        );
        assert_eq!(s, st);
        assert!(!fx.iter().any(|e| matches!(e, Effect::HideClassify)));

        let (s2, fx2) = next(st, &Event::ClassifyDone(3200), &c);
        assert_eq!(
            s2,
            State::Started {
                checkin_at: None,
                sample_at: Some(3500),
                off_task_since: None,
                ontask_at: Some(5000),
            }
        );
        assert!(fx2.contains(&Effect::HideClassify));
        assert!(fx2.contains(&Effect::LogEdge { entered: "started", at: 3200, mode: None }));
        assert!(fx2.contains(&Effect::LogOutcome { outcome: Outcome::CheckedIn, at: 3200 }));
    }

    // An ignored classification screen dies at the schedule edge like an
    // ignored list, taking its window with it; a break silences it the same way.
    #[test]
    fn ignored_classifying_dies_and_break_silences_it() {
        let out = ctx(false, Some(Edge { at: 90_000, kind: EdgeKind::TaskStart }));
        let st = State::Classifying { task_id: 7, shown_at: 3100 };
        let (s, fx) = next(st, &Event::EdgeTimer(5000), &out);
        assert_eq!(s, State::Idle);
        assert!(fx.contains(&Effect::HideClassify));

        let c = classify_ctx(window_end(99_999));
        let (s2, fx2) = next(st, &Event::BreakFor(3200, 600), &c);
        assert_eq!(s2, State::Break { resume_at: 3800 });
        assert!(fx2.contains(&Effect::HideClassify));
        // A break from classifying was a live task; it resumes to Started.
        let (s3, _) = next(s2, &Event::EdgeTimer(3800), &c);
        assert!(matches!(s3, State::Started { .. }));
    }

    // Reload re-emits the screen so a restart keeps it up.
    #[test]
    fn reload_reemits_the_classify_screen() {
        let c = classify_ctx(window_end(99_999));
        let st = State::Classifying { task_id: 7, shown_at: 3100 };
        let (s, fx) = next(st, &Event::RulesReloaded(3300), &c);
        assert_eq!(s, st);
        assert!(fx.contains(&Effect::ShowClassify {
            tools: c.classify_tools.clone(),
            task_id: 7
        }));
    }

    // The zero-polling guarantee extends to the quiet states: a paused or
    // on-break machine carries no sample edge to wake on (PLAN §7).
    #[test]
    fn no_sample_edge_while_paused_or_on_break() {
        let c = listed_ctx(window_end(9999));
        for st in [
            State::Paused { resume_at: 2600, was_started: true },
            State::Break { resume_at: 2600 },
        ] {
            let (_, fx) = next(st, &Event::EdgeTimer(2500), &c);
            assert_ne!(armed(&fx).map(|(_, k)| k), Some(EdgeKind::Sample));
        }
    }
}
