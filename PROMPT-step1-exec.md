# Claude Code prompt — Step 1, Phase 1 (run on Opus)

Implement **Phase 1 only** of `PLAN-step1.md`: the pure `task_window.rs` module. Stop after Phase 1 verification passes; do not start Phase 2.

## Read first (only these, in order — don't scan the tree)
- `PLAN-step1.md` §1, §4, §6-Phase-1, §7 — the scope, signature, and budget rules for this work.
- `crates/nudge-core/src/tasks.rs` — the `Task` struct you consume (deadline, minutes, recur, id).
- `crates/nudge-core/src/schedule.rs` lines 1–55 + tests — copy its conventions: injected `now`, no OS/clock calls, table-driven unit tests.
- `crates/nudge-core/src/lib.rs` — `UnixTime`, `EdgeKind`, exports pattern (add `pub mod task_window;`).
- `UI-PLAN.md` §6.8–§6.9 — the horizon table + row-style rules, verbatim source of truth.

## Build
- New file `crates/nudge-core/src/task_window.rs` per PLAN §4 signature: `WindowCfg`, `Row`, `StyleClass`, `display_list(tasks, logged, now, cfg) -> Vec<Row>`.
- Pure: no Win32, no `std::time` clock reads, no I/O. Time enters only as `UnixTime`.
- Encode the §6.8 horizon table verbatim (remaining = estimate − logged, dynamic `logged`; no-estimate → remaining 0 → 48 h). Sort deadline-ascending regardless of admitting horizon. Cap to `cfg.max_rows` (12). §6.9 styles: not-started white/black-outline; red outline when `deadline − now < not_started_red_before_secs` (default 24 h); else `logged/estimate` → band.
- Register in `lib.rs` and add a one-line row to `COMPONENTS.md`.

## Constraints (reject any deviation)
- Zero business logic outside this module — it is the sole owner of select/sort/window/style (PLAN §4, README critical goal).
- Deterministic, pure, 100% unit-testable. No new deps.
- Follow CLAUDE.md: terse, surgical reads only, no global exploration; one NEXTSTEPS step this session.

## Verify (must pass before claiming done)
- `cargo test -p nudge-core` green.
- Table-driven tests hitting every §6.8 boundary (179 vs 180 min, 6 h, 12 h), deadline-ascending sort across mixed horizons, `NotStartedUrgent` flips exactly at the 24 h threshold, `max_rows` cap, no-estimate → 48 h path. Injected `now` only — no wall-clock waits.
- Do not mark the NEXTSTEPS step done or commit; report readiness and stop.
