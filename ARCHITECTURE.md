# ARCHITECTURE.md — nudge-bot High-Level Project Architecture
<!-- Boundary: local-only deterministic task-initiation nudger; budget: <1% avg CPU, <30 MB RSS, zero network I/O, zero polling. -->

## 1. Mission

Local, deterministic background utility that gets a designated task **started** at its scheduled time. Mechanism: a persistent visual anchor prompt at the task-start edge, escalating in visibility on a fixed ladder if not acknowledged, with an optional later check-in ("did you start X?"). All outcomes logged locally.

**Excluded by directive:** all friction/blocking mechanics from the original plan (foreground-app interception, delay panels, unlock challenges, hosts blocking, config-delay meta-friction), cross-device messaging, cloud, telemetry, AI.

## 2. Design Philosophy

- **Deterministic, not adaptive.** Prompts fire exactly as scheduled. No learning, no prediction.
- **Escalation over enforcement.** The cost of ignoring a prompt is that it becomes harder to *not see* — never harder to use the computer. One click/hotkey always acknowledges or snoozes.
- **Persistent visual real estate.** Always-on-top anchor replaces ephemeral notifications ("out of sight, out of mind" is the failure mode being solved).
- **Honest logging.** Acknowledge / snooze / skip / check-in answers append to `sessions.db`. Data is for the user's own review; no gamification, no penalties.

## 3. State Machine

```
              ┌────────────┐
   boot ────▶ │    IDLE    │  timer armed for next task-start edge
              └─────┬──────┘
     task-start edge │
              ┌──────▼──────┐   escalation edges (absolute times, re-armed one at a time)
              │  PROMPTING  │──▶ L0 anchor strip → L1 enlarged/high-contrast →
              └─┬───┬───┬───┘   L2 centered panel + optional sound → repeat L2 at interval
   acknowledge  │   │   │ skip ("not today", logged)
   ("started")  │   │ snooze(N min) ──▶ back to IDLE-like wait, re-prompts at L0
              ┌─▼───▼──────┐
              │  STARTED   │  anchor shrinks to passive intention strip (optional);
              └─────┬──────┘  check-in timer armed if configured
        check-in edge │
              ┌──────▼──────┐
              │  CHECK_IN   │  "Did you start X?" yes/no one-click; answer logged
              └─────┬───────┘
                    ▼ IDLE (timer re-armed for next edge)
```

All transitions are edge-triggered via one waitable timer + window/tray/hotkey events. **No polling loops. No 1 Hz ticks** — escalation is a small set of discrete absolute-time edges.

## 4. Resource-Saving Constraints (load-bearing)

| Constraint | Mechanism |
|---|---|
| Zero polling | `CreateWaitableTimerEx` absolute-time timer armed only for the *next* edge: next task start, next escalation step, snooze expiry, or check-in time — whichever is soonest. Process sleeps in `MsgWaitForMultipleObjects`. |
| Minimal wake-ups | Idle steady state = 0 timers firing except next task-start edge. No foreground hooks anywhere (removed with the pivot). |
| Memory cap | Static rule set loaded once into fixed structs; SQLite (single file, WAL off, small page cache) for outcome log. Target RSS < 30 MB. |
| Battery | Timer coalescing ±30 s on schedule edges (escalation edges may use tighter tolerance while prompting). Overlay repaints only on content/escalation-level change. Sound = one `PlaySound` call per escalation edge, no loop. |
| No resource leaks | Prompt windows and timers created on state entry are destroyed on state exit, paired in the same code path. |

## 5. Reminder Mechanics → Local Implementation Map

| Mechanic | Local implementation | Notes |
|---|---|---|
| Scheduled start prompt | At task-start edge, show anchor strip (existing layered `WS_EX_TOPMOST\|WS_EX_NOACTIVATE` overlay) with task text + `[Start] [Snooze] [Skip]`. | Reuses `overlay.rs`; adds click handling (drop `WS_EX_NOACTIVATE` only while interactive, or hit-test clicks). |
| Escalation ladder | Fixed per-rule ladder, e.g. L0 at T+0, L1 at T+5 min (taller strip, high-contrast color), L2 at T+10 min (centered panel + one sound), L2 repeat every 10 min, ceiling configurable. Each step = one timer edge + one repaint. | Never covers full screen, never steals focus, never blocks input. |
| Acknowledge / snooze / skip | Button click, tray menu item, or global hotkey (`RegisterHotKey`). Snooze re-arms a single edge. All three append a row to `sessions.db`. | One action, zero friction — the opposite of the old challenge mechanic. |
| Check-in | Optional per-rule: N minutes after acknowledge, small anchor asks "Still on X?" yes/no; answer logged. Auto-dismisses at next edge if ignored (logged as no-answer). | Pure timer edge. |
| Activity-informed check-in | At the check-in edge only, svc may query ActivityWatch (localhost:5600) for recent foreground app and pre-fill/skip the question ("VS Code active 18 of last 20 min → auto-log yes"). | Read-at-edge only; no hooks, no continuous monitoring inside nudge-bot. AW down → fall back to plain check-in. |
| Drafted next-steps | Anchor optionally displays contents of `draft.txt` written by the `nudge-draft` companion. svc just reads a file at edge time. | Keeps LLM/network entirely out of the core. |
| Passive intention anchor | After Start, optional slim strip showing the task text for the task window (existing behavior). | Carryover from original design — still valid. |
| Persistence | Task Scheduler logon task, restart-on-failure (existing `register_tasks.ps1`, minus elevated-helper registration). | |
| Hardware trigger | Deferred; hotkey stands in behind `trigger` trait. | Unchanged. |

## 6. Process Topology

```
nudge-svc.exe   user-mode background process (tray icon), owns state machine,
                the single armed timer, prompt/anchor windows, SQLite.
                Single process, single thread + message loop. Rust.
nudge-ctl.exe   CLI for rule edits (format-preserving toml_edit writes, validate-
                before-write, atomic rename), reload/quit signaling, log queries.

Companion processes (not part of the deterministic core):
ActivityWatch   third-party tracker, own process/budget, installed at
                C:\Users\harri\AppData\Local\activitywatch. svc queries its local
                REST API (localhost:5600) or reads its DB ONLY at timer edges
                (blocking read with short timeout; failure = treated as no-data).
nudge-draft.exe (future, optional) reads sessions.db + ActivityWatch history,
                calls an LLM API, writes drafted next-steps to
                %LOCALAPPDATA%\nudge-bot\draft.txt for the anchor to display.
                Only component with internet I/O. Run manually or via
                Task Scheduler; never spawned by svc.
```

`nudge-helper.exe` (elevated) is **removed** — nothing left needs elevation.

## 7. Data Flow

`rules.toml` (read at boot / on reload signal) → schedule.rs computes next edge → state machine → prompt windows → user action events → SQLite `sessions.db` (append-only: prompts shown, escalation level reached, acknowledge/snooze/skip, check-in answers) → `nudge-ctl log` read views. IPC = two named events (quit, reload) only.

Companion flow: ActivityWatch DB/API ──(read at edges)──▶ nudge-svc; sessions.db + AW history ──▶ nudge-draft ──▶ `draft.txt` ──(file read at edges)──▶ anchor display.
