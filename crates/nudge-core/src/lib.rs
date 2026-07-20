//! nudge-core: pure, deterministic nudge logic. No OS calls anywhere in this crate.
//! Time enters only as `UnixTime` arguments; effects leave only as data.

pub mod escalate;
pub mod rules;
pub mod schedule;
pub mod state;
pub mod task_window;
pub mod tasks;

/// Seconds since Unix epoch, injected by the caller (svc owns the clock).
pub type UnixTime = i64;

/// What the svc's single armed timer represents. `schedule::context` emits only
/// the two schedule-derived kinds (`TaskStart`, `WindowEnd`); the runtime kinds
/// (`EscalationStep`, `SnoozeExpiry`, `CheckIn`) are produced by the state
/// machine (escalate ladder / snooze / check-in) and merged with the schedule
/// edge — the earliest wins — when 2c wires them in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// A nudge window opened: the user should START this task now → begin prompting.
    TaskStart,
    /// A nudge window closed: drop any active prompt for it.
    WindowEnd,
    /// An escalation-ladder step fell due (level rise or L2 re-alert).
    EscalationStep,
    /// A snooze period expired: re-show the prompt.
    SnoozeExpiry,
    /// A post-ack check-in came due ("still on X?").
    CheckIn,
    /// A STARTED-mode AW sample fell due (§6.1): probe the foreground app to see
    /// whether the user is still on-task. Armed *only* while `Started`-unpaused —
    /// structurally absent (`sample_at == None`) everywhere else, so an idle or
    /// paused machine has no sample edge to wake on (zero-polling, PLAN §7).
    Sample,
    /// A §6.4 on-task check-in tick fell due: if the foreground app is off
    /// EVERY due-window task's tools, ask "what are you working on?"; else
    /// silently re-arm the next tick (the cadence is a floor, not a metronome).
    OnTaskCheckIn,
    /// A tray Pause (§6.6) expired: resume supervision. While paused this is the
    /// *only* edge armed — the schedule edge is deliberately not merged, so
    /// nothing can fire mid-pause; it is recomputed fresh on resume.
    PauseExpiry,
    /// A §6.5 Take-a-break expired: resume the task. Same single-edge silencing
    /// as `PauseExpiry`, kept distinct so the outcomes log can tell a chosen
    /// break from a tray pause.
    BreakExpiry,
}

impl EdgeKind {
    /// Stable label for the sessions log (records which timer the svc armed
    /// next when it wrote a state edge).
    pub fn label(&self) -> &'static str {
        match self {
            EdgeKind::TaskStart => "task_start",
            EdgeKind::WindowEnd => "window_end",
            EdgeKind::EscalationStep => "escalation_step",
            EdgeKind::SnoozeExpiry => "snooze_expiry",
            EdgeKind::CheckIn => "check_in",
            EdgeKind::Sample => "sample",
            EdgeKind::OnTaskCheckIn => "ontask_check_in",
            EdgeKind::PauseExpiry => "pause_expiry",
            EdgeKind::BreakExpiry => "break_expiry",
        }
    }
}

/// The next timer edge: an absolute time plus what firing it means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    pub at: UnixTime,
    pub kind: EdgeKind,
}

/// Which notification mode a prompt should render in (UI-PLAN §1). Decided at
/// the edge by comparing the current foreground app to the user's "productive
/// apps" list: on a match the user is plainly already working → `OnTask` (soft,
/// peripheral, no escalation); otherwise `OffTask` (strong, centered, may
/// escalate). AW-down / no foreground signal defaults to `OffTask`, matching the
/// plan's "AW down → default OFF-TASK" — better to over-nudge than to stay
/// silent when we can't tell. Core stays pure: it only branches on this
/// caller-supplied signal; the svc owns the AW probe and the app matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Off-task (or unknown): attention-capturing, escalates as designed.
    #[default]
    OffTask,
    /// On-task: minimal-interruption peripheral cue, no escalation.
    OnTask,
}

impl Mode {
    /// Stable label for the sessions log.
    pub fn label(&self) -> &'static str {
        match self {
            Mode::OffTask => "off_task",
            Mode::OnTask => "on_task",
        }
    }
}

/// Whether the user is clearly at the keyboard, as far as the svc could tell
/// from ActivityWatch at a check-in edge. Lets an activity-informed check-in
/// resolve itself (`Active` → the user is obviously working, skip the nag)
/// instead of always prompting. The svc probes AW only at the check-in edge and
/// applies a staleness guard; every other transition sees `Unknown`, which
/// falls back to the visible prompt. Core stays pure — it never reads AW, it
/// only branches on this caller-supplied signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Presence {
    /// No signal: AW unreachable, snapshot stale, or not a check-in edge.
    #[default]
    Unknown,
    /// AW reports the user actively at the machine (fresh "not-afk").
    Active,
    /// AW reports the user away ("afk").
    Away,
}
