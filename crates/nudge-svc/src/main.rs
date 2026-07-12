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
mod timers;
mod tray;

use nudge_core::state::{next, Effect, Event, State};

/// Reject an AW snapshot whose latest afk event ended more than this long before
/// `now` — a watcher that stopped can't vouch for "not-afk". Generous enough to
/// cover AW's poll cadence, tight enough that a truly idle machine still nags.
const CHECKIN_STALENESS_SECS: i64 = 180;

/// Does the pending event land on a due check-in edge? Only then does the svc
/// pay for an AW probe — every other transition sees `Presence::Unknown`.
fn checkin_due(state: State, now: i64, event: &Event) -> bool {
    matches!(event, Event::EdgeTimer(_) | Event::RulesReloaded(_))
        && matches!(state, State::Started { checkin_at: Some(t) } if now >= t)
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

/// Executes effects emitted by the pure state machine. This is the only
/// place OS resources are created/destroyed — paired per transition.
fn run_effects(effects: Vec<Effect>, res: &mut Resources) {
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
            Effect::ShowPrompt { text, level, mode } => match &mut res.overlay {
                Some(a) => a.update(&text, level, mode),
                None => res.overlay = Some(overlay::Anchor::create(&text, level, mode, res.geom)),
            },
            Effect::HidePrompt => drop(res.overlay.take()),
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
    run_effects(fx, &mut res);

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
        let mut ctx = nudge_core::schedule::context_with_tasks(&rules, &res.db.tasks(), now);
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
        let (s, fx) = next(state, &event, &ctx);
        state = s;
        run_effects(fx, &mut res);
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
