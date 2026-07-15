# COMPONENTS.md — File Component Registry
<!-- Boundary: build manifest; every module must honor the <1% CPU / <30 MB RSS / zero-polling budget in ARCHITECTURE.md; no blocking/friction mechanics. -->

Target stack: **Rust** (release, LTO, no async runtime — plain Win32 message loop).
Toolchain: Rust 1.97 + VS2022 C++ build tools (MSVC target requires the C++ workload).

## Directory Layout

```
nudge-bot/
├── ARCHITECTURE.md
├── COMPONENTS.md
├── README.md
├── NEXTSTEPS.md
├── rules.example.toml
├── Cargo.toml                 # workspace: core, svc, ctl
├── crates/
│   ├── nudge-core/            # pure logic, no OS calls (unit-testable)
│   │   ├── src/lib.rs
│   │   ├── src/state.rs
│   │   ├── src/rules.rs
│   │   ├── src/schedule.rs
│   │   └── src/escalate.rs    # NEW: escalation ladder as pure edge computation
│   ├── nudge-svc/             # background service binary
│   │   ├── src/main.rs
│   │   ├── src/timers.rs
│   │   ├── src/overlay.rs     # extend: interactive prompt levels L0–L2
│   │   ├── src/sound.rs       # NEW: single PlaySound per escalation edge
│   │   ├── src/tray.rs
│   │   ├── src/hotkey.rs
│   │   ├── src/shutdown.rs
│   │   ├── src/reload.rs
│   │   └── src/persist.rs
│   ├── nudge-ctl/             # CLI binary
│   │   └── src/main.rs
│   ├── nudge-app/             # NEW: on-demand planner/settings/calendar GUI (Tauri v2).
│   │   ├── src-tauri/         # Rust backend: shares nudge-core; tasks-db writer;
│   │   │                      # Google OAuth/Calendar client (only GUI-side network).
│   │   └── src/               # Frontend (Svelte): 3-pane layout, Planner/Calendar/
│   │                          # Triggers/Settings/Log tabs, pinned-deadline sidebar.
│   └── nudge-draft/           # FUTURE optional companion: LLM next-step drafts.
│       └── src/main.rs        # Internet I/O allowed here and in nudge-app only.
├── scripts/
│   ├── register_tasks.ps1     # logon task only (elevated-helper registration REMOVED)
│   └── uninstall.ps1          # unconditional teardown (config-delay refusal REMOVED)
└── tests/
    └── core_integration.rs
```

## Module Responsibilities

### crates/nudge-core (no Win32 imports; deterministic; 100% unit-testable)
| File | Responsibility |
|---|---|
| `state.rs` | State enum (IDLE/PROMPTING/STARTED/CHECK_IN) + pure `next(state, event) -> (state, effects)`. Events: TimerEdge, Ack, Snooze(min), Skip, CheckInYes/No, RulesReloaded, HotkeyToggle. Effects are data, executed by svc. **STARTED sampling (§6.1, Phase 3):** `Started` carries `sample_at` (next AW sample edge) + `off_task_since` (start of the current continuous off-task run). `sample_at` is `Some` **only** in `Started` with sampling configured — no other state can arm a `Sample` edge because none carries one, which is the zero-polling guarantee structurally rather than by a guard. A continuous off-task run ≥ `off_task_secs` raises the drift check-in; returning on-task resets it. `Started`'s two runtime edges (check-in, sample) collapse to their earliest in `arm_started` before merging with the schedule edge — still exactly one armed timer. |
| `rules.rs` | Parse/validate `rules.toml`: tasks (name, days, start time, prompt text), per-task escalation ladder (step offsets, max level, repeat interval, sound on/off), snooze default, check-in offset, anchor style. Fixed-size structs. No blocklists, no delay/unlock fields. |
| `schedule.rs` | Given rules + now, compute **next edge timestamp** (single value the svc arms a timer for). Unchanged core; edge kinds extended (TaskStart / EscalationStep / SnoozeExpiry / CheckIn). |
| `escalate.rs` | Pure ladder: `(rule, prompt_shown_at, now) -> (level, next_step_at)`. Deterministic; no timers of its own. |
| `task_window.rs` | Pure `display_list(tasks, logged, now, cfg) -> Vec<Row>` (§6.8 dynamic-deadline horizon + §6.9 row styling). Sole owner of task-list select/sort/window/style; svc popups + app `TaskListPanel` consume its rows verbatim. No OS/clock/I/O. |

### crates/nudge-svc
| File | Responsibility |
|---|---|
| `main.rs` | Boot: load rules, restore state from SQLite, arm next-edge timer, enter message loop. Owns teardown of all handles. Edge-gated AW probes: `checkin_due` (presence), `taskstart_due` (mode), `sample_due` (§6.1 foreground drift). `foreground_on_task()` does the sample compare — the task's own `task_tools` list first, else global `[classify] productive_apps`; `ignore`-kind tools count as on-task; AW-down or nothing-configured reads as on-task. On an on-task sample it folds one cadence into `logged_minutes` via `set_logged_minutes` (the sanctioned svc→tasks write, §3) — lazy at the edge, never ticked. |
| `timers.rs` | `CreateWaitableTimerEx` wrapper, absolute-time, coalescing 30 s idle / tighter while PROMPTING. Exactly **one** timer armed at any moment (next edge of any kind). Wait set: `[timer, quit, reload]`. |
| `overlay.rs` | Anchor/prompt window, two render modes (UI-PLAN.md §1). OFF-TASK: centered high-contrast panel, one-shot slide-in, escalation L0–L2. ON-TASK: slim muted peripheral strip, no motion, auto-fade. Buttons `[Start] [Snooze] [Skip]` hit-tested; click-through launches/focuses nudge-app at the task page. Never full-screen, never steals focus, never blocks input. Destroyed outside PROMPTING/STARTED. |
| `sound.rs` | One `PlaySound(SND_ASYNC)` per qualifying escalation edge. No loops, no mixer state held. |
| `tray.rs` | tray-icon crate. Menu: status, Start now (ack), Snooze, Skip today, Toggle anchor, Reload rules, Quit. |
| `hotkey.rs` | `RegisterHotKey` — Ctrl+Alt+N acknowledges current prompt / toggles anchor. Trigger trait for future hardware. |
| `aw_query.rs` | At trigger + check-in **+ sample** edges only: GET `localhost:5600/api/0/...` (minimal blocking HTTP, short timeout, e.g. `ureq` or raw winhttp — no async runtime) or read AW's SQLite directly (`C:\Users\harri\AppData\Local\activitywatch`). Drives on/off-task mode pick (foreground app vs. productive-app list). AW absent/down = default OFF-TASK mode + plain check-in; at a sample edge, AW-down reads as **on-task** (no signal must never manufacture drift). |
| `shutdown.rs` | Named quit event + console ctrl handler (existing, verified). |
| `reload.rs` | Named reload event (existing, verified). |
| `persist.rs` | SQLite: `sessions` rows for prompt_shown, mode fired (on/off-task + classification result), escalation_level_reached, ack, snooze, skip, checkin_answer, click_through. `tasks` table (title, desc, deadline, type, recur, trigger mode, source: manual/gmail/gcal, gcal_event_id, **`estimate_minutes`/`logged_minutes` §6.3**) — svc reads read-only EXCEPT the sanctioned single-column `logged_minutes` UPDATE-by-rowid (`set_logged_minutes`, §3); nudge-app is the writer of every other column; reload event signals changes. `task_tools(task_id, app_name, kind)` shared reader table (svc reads at Phase-3 sample compare via `task_tools()`), byte-identical with the app schema. Global `app_classes(app_name, class)` (§6.2, `app_class()` reader) + `app_usage(app_name, minutes_90d, refreshed_at)` selector cache — both app-owned, svc read-only. Additive Phase-2 migration via ALTER-ignore, stamped `PRAGMA user_version = 2`. Drop `unlocks`/`pending_changes`. |

### crates/nudge-ctl (CLI)
| File | Responsibility |
|---|---|
| `main.rs` | `status`, `anchor <text>`, `nudge ls|add|rm` (existing, format-preserving toml_edit), new: `escalation set`, `checkin set`, `ack`/`snooze`/`skip` (signal svc via named event), `log`, `reload`, `quit`. All edits apply immediately — no delay queue. |

### scripts/
| File | Responsibility |
|---|---|
| `register_tasks.ps1` | Register svc logon task with restart-on-failure; seed rules.toml. Remove elevated-helper task registration and ACL locking. |
| `uninstall.ps1` | Unregister task, remove config. No hosts cleanup, no challenge gate. |

## Removed / Deferred (wrong-problem components — do not rebuild)

| Component | Status |
|---|---|
| `nudge-core/src/delay_queue.rs` | **Delete.** Config-delay meta-friction dropped; edits apply immediately. |
| `nudge-core/src/challenge.rs` | **Delete.** No unlock challenges. |
| `nudge-svc/src/fg_hook.rs` | **Delete (never implemented).** No foreground-app interception. |
| `nudge-svc/src/delay_panel.rs` | **Delete (never implemented).** No countdown blocking. |
| `crates/nudge-helper/` + `helper_ipc.rs` | **Delete.** No hosts edits, no elevation needed. |
| Hosts-file blocking, `Frozen Turkey` lockout, unlock budget | **Out of scope permanently.** |

## Connection Contract

- svc is the only writer of state; core is pure; ctl writes rules (validated, atomic) and signals named events.
- **`tasks` writer split (§3):** nudge-app owns every `tasks` column and the app-only tables (`app_classes`, `app_usage` classification/usage, plus `task_tools`). The svc's *only* sanctioned write into `tasks` is `logged_minutes` (single UPDATE by rowid, `persist::set_logged_minutes`) — any other svc→`tasks` write is a contract violation. `task_tools` is created byte-identical in both `persist.rs` and `db.rs` because the svc reads it at the sample edge.
- Every effect (timer arm, window create) has a paired teardown in the same state-exit path.
- No thread pools, no async runtime, no background tick. Any PR introducing a periodic timer with period < next-edge, or any mechanic that blocks/intercepts user activity, must be rejected.
- **Named-event literals (`Local\nudge-bot-quit`, `Local\nudge-bot-reload`) are duplicated as string
  constants in both nudge-svc and nudge-ctl — there is no shared crate for them. If you rename or add
  one, grep both crates; a silent mismatch means ctl's signal is a no-op.
