//! nudge-svc: single-process, single-thread background service.
//! Boot: load rules -> restore state -> arm next-edge timer -> GetMessage loop.
//! No async runtime, no thread pool, no background tick.

mod aw_query;
mod draft;
mod hotkey;
mod launch;
mod overlay;
mod persist;
mod reload;
mod shutdown;
mod sound;
mod tasklist;
mod timers;
mod tray;

use nudge_core::state::{next, CheckInKind, Effect, Event, State};
use nudge_core::task_window::{display_list, LoggedMap, Progress, WindowCfg};
use nudge_core::tasks::Task;

/// Reject an AW snapshot whose latest afk event ended more than this long before
/// `now` — a watcher that stopped can't vouch for "not-afk". Generous enough to
/// cover AW's poll cadence, tight enough that a truly idle machine still nags.
const CHECKIN_STALENESS_SECS: i64 = 180;

/// Does the pending event land on a due check-in edge? Only then does the svc
/// pay for an AW probe — every other transition sees `Presence::Unknown`.
fn checkin_due(state: State, now: i64, event: &Event) -> bool {
    matches!(event, Event::EdgeTimer(_) | Event::RulesReloaded(_))
        && matches!(state, State::Started { checkin_at: Some(t), .. } if now >= t)
}

/// Does the pending event land on a due STARTED sample edge (§6.1)? A sample edge
/// only exists while a task is live and sampling is configured, so this is the one
/// place drift detection costs an AW probe — there is no background tick to guard
/// against, because outside `Started` there is no edge to fire.
fn sample_due(state: State, now: i64, event: &Event) -> bool {
    matches!(event, Event::EdgeTimer(_) | Event::RulesReloaded(_))
        && matches!(state, State::Started { sample_at: Some(t), .. } if now >= t)
}

/// Does the pending event land on a due §6.4 on-task check-in tick? Only then
/// does the svc pay for the union-of-all-tools compare — every other transition
/// leaves `any_task_on_task` at its safe `true` default.
fn ontask_due(state: State, now: i64, event: &Event) -> bool {
    matches!(event, Event::EdgeTimer(_) | Event::RulesReloaded(_))
        && matches!(state, State::Started { ontask_at: Some(t), .. } if now >= t)
}

/// Is the foreground app in the tool list of ANY task in the dynamic deadline
/// window (§6.4 broadened compare)? `rows` are the `display_list` output — the
/// same "due window" the check-in's list will show.
///
/// Mirrors [`foreground_on_task`]'s never-nag-on-no-signal reads: no foreground
/// app (AW down) → `true`; no tool configured on any due task → fall back to
/// the global productive-app list, and if that too is empty → `true`.
fn any_task_on_task(
    db: &persist::Db,
    rules: &nudge_core::rules::Rules,
    rows: &[nudge_core::task_window::Row],
    app: Option<&str>,
) -> bool {
    let Some(app) = app else { return true };
    let mut any_tools = false;
    for r in rows {
        let tools = db.task_tools(r.task_id);
        if tools.is_empty() {
            continue;
        }
        any_tools = true;
        if tools.iter().any(|(name, _kind)| name.eq_ignore_ascii_case(app)) {
            return true;
        }
    }
    if !any_tools {
        if rules.classify.productive_apps.is_empty() {
            return true;
        }
        return rules.classify.mode(Some(app)) == nudge_core::Mode::OnTask;
    }
    false
}

/// Is the foreground app one of the live task's tools (§6.1 sample compare)?
///
/// Precedence: the task's own `task_tools` list, else the global
/// `[classify] productive_apps` list — so sampling is useful before the per-task
/// tools selector (Tier B) exists. An `ignore`-kind tool counts as on-task: it's a
/// task-scoped transient the user already told us not to be nagged about.
///
/// Two cases deliberately read as on-task rather than drift: no foreground signal
/// at all (AW down — PLAN §7 says treat no-data as on-task), and no configured
/// notion of on-task anywhere (empty tool list *and* empty productive-app list),
/// which would otherwise make every single app count as drift and nag forever.
fn foreground_on_task(
    db: &persist::Db,
    rules: &nudge_core::rules::Rules,
    task_id: Option<i64>,
    app: Option<&str>,
) -> bool {
    let Some(app) = app else { return true };
    let tools = task_id.map(|id| db.task_tools(id)).unwrap_or_default();
    if tools.is_empty() {
        if rules.classify.productive_apps.is_empty() {
            return true;
        }
        return rules.classify.mode(Some(app)) == nudge_core::Mode::OnTask;
    }
    tools.iter().any(|(name, _kind)| name.eq_ignore_ascii_case(app))
}

/// Is a task-window prompt about to open (Idle → Prompting)? Mode is decided
/// once here, at the edge, so the svc pays for at most one AW foreground probe
/// per window opening — not on every escalation tick. Boot/reload landing
/// mid-window counts (same Idle→Prompting transition the state machine takes).
fn taskstart_due(state: State, in_window: bool, event: &Event) -> bool {
    in_window
        && matches!(state, State::Idle)
        && matches!(event, Event::EdgeTimer(_) | Event::RulesReloaded(_))
}

/// Is a drift check-in on screen, i.e. could the very next event be the No that
/// brings the task list up? Only then is the §6.5 list worth computing — the
/// answer has to be ready before `next()` runs, and every other transition would
/// be paying for a list nobody asked for.
fn task_list_due(state: State) -> bool {
    matches!(
        state,
        State::CheckIn { kind: CheckInKind::OffTask | CheckInKind::OnTask, .. }
            | State::Choosing { .. }
    )
}

/// Per-task progress for `display_list`, read straight off the `tasks` rows the
/// svc already loaded this wake (`logged_minutes` is the cache it maintains at
/// sample edges, so no AW re-scan happens here).
fn progress_map(tasks: &[Task]) -> LoggedMap {
    tasks
        .iter()
        .filter_map(|t| {
            Some((
                t.id?,
                Progress { logged: t.logged_minutes, estimate: t.estimate_minutes },
            ))
        })
        .collect()
}

/// Executes effects emitted by the pure state machine. This is the only
/// place OS resources are created/destroyed — paired per transition.
fn run_effects(effects: Vec<Effect>, now: i64, res: &mut Resources) {
    // A batch is one transition. The `ArmEdgeTimer` in it names the next edge the
    // svc will wake on; pre-scan it so the state row logged in the same batch can
    // record which kind was armed (routes ArmEdgeTimer `kind` into logging).
    let armed_kind = effects.iter().find_map(|e| match e {
        Effect::ArmEdgeTimer { kind, .. } => Some(kind.label()),
        _ => None,
    });
    for fx in effects {
        match fx {
            // Render at the escalation level; update the live strip in place
            // (no destroy/recreate flicker) or create it on first show.
            Effect::ShowPrompt { text, level, mode, buttons } => match &mut res.overlay {
                Some(a) => a.update(&text, level, mode, buttons),
                None => {
                    res.overlay =
                        Some(overlay::Anchor::create(&text, level, mode, buttons, res.geom))
                }
            },
            Effect::HidePrompt => drop(res.overlay.take()),
            // Rows arrive fully selected/sorted/styled from `task_window`; the
            // window below only paints them.
            Effect::ShowTaskList { rows } => {
                res.tasklist = Some(tasklist::TaskList::create(&rows, now))
            }
            Effect::HideTaskList => drop(res.tasklist.take()),
            Effect::PlaySound => sound::alert(),
            Effect::ArmEdgeTimer { at, kind: _ } => res.edge_timer.arm_absolute(at),
            Effect::LogEdge { entered, at, mode } => {
                res.db.log_edge(entered, at, mode.map(|m| m.label()), armed_kind)
            }
            Effect::LogOutcome { outcome, at } => res.db.log_outcome(outcome.label(), at),
        }
    }
}

struct Resources {
    overlay: Option<overlay::Anchor>,
    /// The §6.5 task list, up only between a check-in's No and its resolution.
    tasklist: Option<tasklist::TaskList>,
    /// Strip geometry from `[anchor]`; refreshed on every rules load so a live
    /// reload re-sizes/re-docks the next prompt.
    geom: overlay::AnchorGeom,
    edge_timer: timers::EdgeTimer,
    db: persist::Db,
}

/// Shipped default config, embedded at compile time. Seeded to disk on first
/// run so a bare launch never fails on a missing rules.toml.
const RULES_TEMPLATE: &str = include_str!("../../../rules.example.toml");

fn main() {
    let config = dirs_config();
    let rules_path = config.join("rules.toml");
    // Where `nudge-draft` (step 7c-ii) writes its LLM next-step suggestion; the
    // svc reads it each transition to override the static window text.
    let draft_path = config.join("draft.txt");

    // First-run seeding: ensure the config dir exists and drop in the default
    // rules.toml if absent. Fixes the "rules.toml missing / path not found"
    // panic when the %LOCALAPPDATA%\nudge-bot dir was never created.
    if let Err(e) = std::fs::create_dir_all(&config) {
        panic!("cannot create config dir {}: {e}", config.display());
    }
    if !rules_path.exists() {
        std::fs::write(&rules_path, RULES_TEMPLATE)
            .unwrap_or_else(|e| panic!("cannot seed {}: {e}", rules_path.display()));
        eprintln!("seeded default rules at {}", rules_path.display());
    }

    let src = std::fs::read_to_string(&rules_path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", rules_path.display()));
    let mut rules = nudge_core::rules::parse(&src)
        .unwrap_or_else(|e| panic!("invalid {}: {e:?}", rules_path.display()));

    let mut res = Resources {
        overlay: None,
        tasklist: None,
        geom: overlay::AnchorGeom::from_rules(&rules.anchor),
        edge_timer: timers::EdgeTimer::new(),
        db: persist::Db::open(dirs_config().join("sessions.db")),
    };
    let mut state = State::Idle;

    // Boot probe: log whether ActivityWatch is reachable and what it sees.
    // Check-in edges re-probe live (see `checkin_due`); this one surfaces AW
    // availability at startup so a misconfigured AW is obvious in the log.
    let act = aw_query::probe();
    eprintln!(
        "activitywatch: afk={:?} app={:?} title={:?}",
        act.afk, act.app, act.title
    );

    let tray = tray::Tray::install();
    let quit = shutdown::QuitSignal::install();
    let reload = reload::ReloadSignal::install();
    let _hotkey = rules.hotkey.as_ref().map(hotkey::Toggle::register);

    // Initial evaluation: enter correct state for boot time and arm the edge.
    let now = timers::local_now();
    let boot_event = Event::RulesReloaded(now.unix);
    // App-created triggers: read the `tasks` table (svc is the read-only
    // consumer; nudge-app writes and signals via `Local\nudge-bot-reload`) and
    // fold their recurring windows in alongside the rules `[[nudge]]` blocks.
    let mut ctx = nudge_core::schedule::context_with_tasks(&rules, &res.db.tasks(), now);
    if ctx.in_window {
        if let Some(text) = draft::read(&draft_path, now.unix, draft::DRAFT_STALENESS_SECS) {
            ctx.window_text = text;
        }
    }
    if taskstart_due(state, ctx.in_window, &boot_event) {
        ctx.mode = ctx
            .window_mode_override
            .unwrap_or_else(|| rules.classify.mode(aw_query::probe().app.as_deref()));
        eprintln!("nudge mode: {}", ctx.mode.label());
    }
    let (s, fx) = next(state, &boot_event, &ctx);
    state = s;
    run_effects(fx, now.unix, &mut res);

    // §6.4 "tools since the last check-in" accumulator (PLAN-step3 §1 option A):
    // each due sample edge pushes the probed foreground app in; any check-in
    // resolution clears it. Runtime scratch, deliberately not core state — P2's
    // classification screen is the consumer; P1 only fills it.
    let mut seen_tools: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();

    // Message loop: wakes only on edge timer, quit event, hotkey, or tray
    // messages.
    let timer_handle = res.edge_timer.raw();
    timers::message_loop(timer_handle, quit.raw(), reload.raw(), &tray, |signal| {
        let now = timers::local_now();
        let event = match signal {
            timers::LoopSignal::OpenApp => {
                // Notification click-through (P1): focus/launch the GUI and mark
                // the click on the current prompt's session row. The live
                // `Prompting` window carries its `tasks` rowid (9d-ii threading),
                // so target that task's page; any other state (or a rules-only
                // window) hands `None` and opens to the default page.
                launch::open_app(state.task_id());
                res.db.log_clickthrough(now.unix);
                return;
            }
            timers::LoopSignal::Core(e) => e,
            // Pause/break durations live in rules, which the message loop doesn't
            // hold; stamp them on here.
            timers::LoopSignal::Pause(t) => Event::PauseFor(t, rules.escalation.pause_secs),
            timers::LoopSignal::Break(t) => Event::BreakFor(t, rules.escalation.break_secs),
            timers::LoopSignal::Reload => {
                // Re-read rules.toml from disk. On any read/parse error, keep the
                // running rules and skip the re-evaluation (last good config stays
                // live). Hotkey binding is fixed at boot — a changed hotkey needs a
                // restart.
                match std::fs::read_to_string(&rules_path)
                    .map_err(|e| e.to_string())
                    .and_then(|src| nudge_core::rules::parse(&src).map_err(|e| format!("{e:?}")))
                {
                    Ok(new_rules) => {
                        rules = new_rules;
                        res.geom = overlay::AnchorGeom::from_rules(&rules.anchor);
                        Event::RulesReloaded(now.unix)
                    }
                    Err(msg) => {
                        eprintln!("reload rejected, keeping current rules: {msg}");
                        return;
                    }
                }
            }
        };
        // Re-read the `tasks` table each wake so app edits (signaled via reload)
        // take effect; wakes are edge-only, so this stays off the hot path.
        let tasks = res.db.tasks();
        let mut ctx = nudge_core::schedule::context_with_tasks(&rules, &tasks, now);
        // LLM draft override: inside a task window, a fresh `draft.txt` next-step
        // replaces the static window text (and thus the check-in text too). A
        // missing/blank/stale draft leaves the rules text untouched.
        if ctx.in_window {
            if let Some(text) = draft::read(&draft_path, now.unix, draft::DRAFT_STALENESS_SECS) {
                ctx.window_text = text;
            }
        }
        // Activity-informed check-in: only when a check-in is actually due do we
        // probe AW, so an active user's nag can resolve itself silently.
        if checkin_due(state, now.unix, &event) {
            ctx.presence = aw_query::probe().presence(now.unix, CHECKIN_STALENESS_SECS);
        }
        // On/off-task mode: pick once as a window's prompt opens (UI-PLAN §1). A
        // task's own `mode_override` wins outright (9f); otherwise classify the
        // current foreground app against the productive-app list (rules-side
        // fallback, also used for rules-only windows with no task).
        if taskstart_due(state, ctx.in_window, &event) {
            ctx.mode = ctx
                .window_mode_override
                .unwrap_or_else(|| rules.classify.mode(aw_query::probe().app.as_deref()));
            eprintln!("nudge mode: {}", ctx.mode.label());
        }
        // STARTED sampling (§6.1): at a due sample edge — and only there — read the
        // foreground app so core can extend or reset the off-task run, then fold
        // the elapsed cadence into `logged_minutes` when the user was on-task.
        // That write is the one sanctioned svc→tasks exception (PLAN §3): a single
        // UPDATE by rowid at an edge, so `logged_minutes` stays a lazily-refreshed
        // cache and is never ticked.
        if sample_due(state, now.unix, &event) {
            let app = aw_query::probe().app;
            if let Some(a) = &app {
                seen_tools.insert(a.clone());
            }
            ctx.foreground_on_task =
                foreground_on_task(&res.db, &rules, ctx.window_task_id, app.as_deref());
            if ctx.foreground_on_task {
                if let (Some(id), Some(secs)) = (ctx.window_task_id, ctx.sample_secs) {
                    let logged = tasks.iter().find(|t| t.id == Some(id)).map_or(0, |t| t.logged_minutes);
                    res.db.set_logged_minutes(id, logged + (secs / 60) as u32);
                }
            }
        }
        // §6.4 on-task tick: at a due tick — and only there — compare the
        // foreground app against the union of every due-window task's tools, so
        // core can decide "drifted off everything → ask" vs "on something → stay
        // quiet" without ever seeing an app name.
        if ontask_due(state, now.unix, &event) {
            let app = aw_query::probe().app;
            if let Some(a) = &app {
                seen_tools.insert(a.clone());
            }
            let rows = display_list(&tasks, &progress_map(&tasks), now.unix, &WindowCfg::default());
            ctx.any_task_on_task = any_task_on_task(&res.db, &rules, &rows, app.as_deref());
        }
        // §6.5 list: computed here, by `task_window::display_list`, so that core
        // can hand it straight to the render on a No without either end selecting,
        // sorting or styling anything (UI-PLAN §6.9, the one owner).
        if task_list_due(state) {
            ctx.task_rows = display_list(&tasks, &progress_map(&tasks), now.unix, &WindowCfg::default());
        }
        let was_asking = matches!(state, State::CheckIn { .. } | State::Choosing { .. });
        let (s, fx) = next(state, &event, &ctx);
        state = s;
        // A check-in resolved (answered, timed out, or silenced): the "since the
        // last check-in" window restarts, so the accumulator empties with it.
        if was_asking && !matches!(state, State::CheckIn { .. } | State::Choosing { .. }) {
            seen_tools.clear();
        }
        run_effects(fx, now.unix, &mut res);
    });

    // Loop returned (tray Quit, quit event, or WM_QUIT). Tear down OS resources
    // on this creating thread — tray icon (NIM_DELETE) and overlay window —
    // then release any console handler blocking the shutdown grace window.
    drop(tray);
    drop(res);
    quit.mark_done();
}

fn dirs_config() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("nudge-bot")
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULES_BARE: &str = "[anchor]\ndefault_text = \"x\"\n";

    fn rules_with_productive(apps: &str) -> nudge_core::rules::Rules {
        let src = format!("{RULES_BARE}[classify]\nproductive_apps = [{apps}]\n");
        nudge_core::rules::parse(&src).unwrap()
    }

    /// A temp sessions.db seeded with raw `task_tools` rows (nudge-app is the
    /// writer in production, so the test stands in for it against the same schema).
    fn db_with_tools(tag: &str, rows: &str) -> (persist::Db, std::path::PathBuf) {
        let mut p = std::env::temp_dir();
        p.push(format!("nudge-svc-test-{}-{tag}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let db = persist::Db::open(p.clone());
        if !rows.is_empty() {
            let conn = rusqlite::Connection::open(&p).unwrap();
            conn.execute_batch(rows).unwrap();
        }
        (db, p)
    }

    fn task_row(id: Option<i64>) -> Task {
        Task {
            id,
            title: "t".into(),
            desc: String::new(),
            deadline: None,
            task_type: String::new(),
            minutes: None,
            recur: nudge_core::tasks::Recur::Once,
            mode_override: None,
            trigger_source: nudge_core::tasks::TriggerSource::Manual,
            gcal_event_id: None,
            estimate_minutes: None,
            logged_minutes: 0,
        }
    }

    fn started(sample_at: Option<i64>) -> State {
        State::Started {
            checkin_at: None,
            sample_at,
            off_task_since: None,
            ontask_at: None,
        }
    }

    // The sample probe is paid for only on a due sample edge in Started.
    #[test]
    fn sample_due_gates_the_probe() {
        assert!(sample_due(started(Some(1500)), 1500, &Event::EdgeTimer(1500)));
        assert!(sample_due(started(Some(1500)), 1600, &Event::EdgeTimer(1600)));
        // Not yet due.
        assert!(!sample_due(started(Some(1500)), 1400, &Event::EdgeTimer(1400)));
        // Sampling disabled → no edge to be due.
        assert!(!sample_due(started(None), 9999, &Event::EdgeTimer(9999)));
        // Not Started.
        assert!(!sample_due(State::Idle, 9999, &Event::EdgeTimer(9999)));
        assert!(!sample_due(
            State::CheckIn { shown_at: 1, kind: CheckInKind::Periodic },
            9999,
            &Event::EdgeTimer(9999)
        ));
        // A user event is not a sampling edge.
        assert!(!sample_due(started(Some(1500)), 1600, &Event::Ack(1600)));
    }

    // The §6.5 list is computed only while an answer of No could actually land —
    // a drift check-in, or the list it already opened (which a reload re-emits).
    #[test]
    fn task_list_due_gates_the_display_list() {
        assert!(task_list_due(State::CheckIn { shown_at: 1, kind: CheckInKind::OffTask }));
        assert!(task_list_due(State::Choosing { shown_at: 1 }));
        // The periodic check-in takes Start/Skip, not Yes/No — no list to build.
        assert!(!task_list_due(State::CheckIn { shown_at: 1, kind: CheckInKind::Periodic }));
        assert!(!task_list_due(State::Idle));
        assert!(!task_list_due(started(Some(1500))));
        assert!(!task_list_due(State::Paused { resume_at: 9, was_started: true }));
    }

    // Progress comes off the rows already in hand: `logged_minutes` is the cache
    // the sample edge maintains, so building the list re-reads nothing.
    #[test]
    fn progress_map_reads_the_logged_cache() {
        let mut t = task_row(Some(7));
        t.logged_minutes = 45;
        t.estimate_minutes = Some(120);
        // An unsaved row has no id, so it cannot key a map — and is skipped.
        let m = progress_map(&[t, task_row(None)]);
        assert_eq!(m.len(), 1);
        assert_eq!(m[&7], Progress { logged: 45, estimate: Some(120) });
    }

    // A task's own tool list decides the compare, and an `ignore` app counts as
    // on-task rather than drift.
    #[test]
    fn task_tools_drive_the_compare() {
        let (db, path) = db_with_tools(
            "tools",
            "INSERT INTO task_tools (task_id, app_name, kind) VALUES
                (1, 'code.exe', 'tool'), (1, 'slack.exe', 'ignore');",
        );
        // The global list would call code.exe off-task; the task list wins.
        let rules = rules_with_productive("\"chrome.exe\"");

        assert!(foreground_on_task(&db, &rules, Some(1), Some("code.exe")));
        assert!(foreground_on_task(&db, &rules, Some(1), Some("CODE.EXE"))); // case-insensitive
        assert!(foreground_on_task(&db, &rules, Some(1), Some("slack.exe"))); // ignore → not drift
        assert!(!foreground_on_task(&db, &rules, Some(1), Some("game.exe")));
        // chrome.exe is globally productive but not one of *this* task's tools.
        assert!(!foreground_on_task(&db, &rules, Some(1), Some("chrome.exe")));

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    // With no per-task tools yet (selector UI is Tier B), the global
    // productive-app list stands in.
    #[test]
    fn falls_back_to_global_productive_apps() {
        let (db, path) = db_with_tools("fallback", "");
        let rules = rules_with_productive("\"code.exe\"");

        assert!(foreground_on_task(&db, &rules, Some(1), Some("code.exe")));
        assert!(!foreground_on_task(&db, &rules, Some(1), Some("game.exe")));
        // A rules-only window (no task id) uses the same fallback.
        assert!(foreground_on_task(&db, &rules, None, Some("code.exe")));
        assert!(!foreground_on_task(&db, &rules, None, Some("game.exe")));

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    // §6.4 broadened compare: on-task means "in ANY due-window task's tools",
    // with the same never-nag fallbacks as the per-task compare.
    #[test]
    fn any_task_on_task_unions_due_window_tools() {
        use nudge_core::task_window::{Row, StyleClass};
        let row = |id: i64| Row {
            task_id: id,
            title: "t".into(),
            deadline: Some(9_000),
            logged: 0,
            estimate: None,
            style: StyleClass::NotStarted,
        };
        let (db, path) = db_with_tools(
            "union",
            "INSERT INTO task_tools (task_id, app_name, kind) VALUES
                (1, 'code.exe', 'tool'), (2, 'word.exe', 'tool');",
        );
        let rules = rules_with_productive("\"chrome.exe\"");
        let rows = [row(1), row(2)];

        // Either task's tool counts; a stranger app does not (and the global
        // list does NOT stand in once any task has tools).
        assert!(any_task_on_task(&db, &rules, &rows, Some("code.exe")));
        assert!(any_task_on_task(&db, &rules, &rows, Some("WORD.EXE")));
        assert!(!any_task_on_task(&db, &rules, &rows, Some("game.exe")));
        assert!(!any_task_on_task(&db, &rules, &rows, Some("chrome.exe")));

        // No tools on any due task → global productive list stands in.
        let bare_rows = [row(3)];
        assert!(any_task_on_task(&db, &rules, &bare_rows, Some("chrome.exe")));
        assert!(!any_task_on_task(&db, &rules, &bare_rows, Some("game.exe")));

        // AW down, or nothing configured anywhere → on-task, never nag.
        assert!(any_task_on_task(&db, &rules, &rows, None));
        let bare = nudge_core::rules::parse(RULES_BARE).unwrap();
        assert!(any_task_on_task(&db, &bare, &bare_rows, Some("anything.exe")));

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    // The on-task probe is paid for only on a due tick in Started.
    #[test]
    fn ontask_due_gates_the_probe() {
        let live = State::Started {
            checkin_at: None,
            sample_at: None,
            off_task_since: None,
            ontask_at: Some(3000),
        };
        assert!(ontask_due(live, 3000, &Event::EdgeTimer(3000)));
        assert!(!ontask_due(live, 2900, &Event::EdgeTimer(2900)));
        assert!(!ontask_due(started(None), 9999, &Event::EdgeTimer(9999)));
        assert!(!ontask_due(live, 3000, &Event::Ack(3000)));
    }

    // The two "we can't tell" paths both read as on-task, so we never nag on a
    // signal we don't have.
    #[test]
    fn unknowable_foreground_reads_as_on_task() {
        let (db, path) = db_with_tools("unknown", "");

        // AW down: no foreground app at all.
        let rules = rules_with_productive("\"code.exe\"");
        assert!(foreground_on_task(&db, &rules, Some(1), None));

        // Nothing configured anywhere: no tool list and no productive apps, so
        // there is no notion of on-task to drift from — every app would otherwise
        // count as drift and nag forever.
        let bare = nudge_core::rules::parse(RULES_BARE).unwrap();
        assert!(foreground_on_task(&db, &bare, Some(1), Some("game.exe")));
        assert!(foreground_on_task(&db, &bare, None, Some("anything.exe")));

        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
