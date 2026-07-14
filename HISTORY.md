# HISTORY.md — Session Log
<!-- Session-by-session history, pulled out of NEXTSTEPS.md to keep that file forward-looking only.
     Repo is live at github.com/harris11ax/nudge-bot-app (see NEXTSTEPS.md "GitHub Workflow"), but as of
     session 40 only the initial commit is actually pushed — this file is still the source of truth for
     everything since, until the user asks to commit/push and optionally migrate this log into Issues/Wiki. -->

## Verified working (sessions 2–7)
- Toolchain: Rust 1.97 + VS2022 C++ build tools. `cargo build --release` clean.
- nudge-svc live: tray (`tray-icon` crate: Toggle anchor / Reload rules / Quit — all exercised),
  anchor overlay (layered topmost strip), Ctrl+Alt+N hotkey, edges → sessions.db.
- Graceful shutdown: `Local\nudge-bot-quit` event; `nudge-ctl quit` → 0 orphans. (`Stop-Process` still orphans tray.)
- Live reload: `Local\nudge-bot-reload` event; `nudge-ctl` edit verbs (`anchor`, `nudge ls|add|rm`, `reload`)
  are format-preserving (toml_edit), validate-before-write, atomic rename, auto-signal running svc.
- Event-name literals duplicated in svc + ctl — **keep in sync** (quit + reload).
- Config at `%LOCALAPPDATA%\nudge-bot\rules.toml`. `scripts\register_tasks.ps1` written, NOT run (autostart off).

## DONE sessions 8–34
- 8–30: escalation ladder + state machine + check-in; nudge-draft LLM crate; on/off-task classification;
  quick-add parser + tasks table + Tauri shell; svc window-gen consumer + click-through launcher.
- 31–33: 9d-ii task-id click-through + 9f mode_override threading. `cargo test --workspace` clean (76).
  Once/deadline-only task firing still deferred (per-task done-flag) — blocks 10e end-to-end.
- 34 (Opus): §10 Calendar+connectors planning run → GOOGLE-PLAN.md, decomposed into 10a–10e. All
  Google network I/O in nudge-app (svc stays offline); PKCE loopback OAuth + DPAPI token cache.

## DONE session 35
- 10a — Google OAuth + token cache: `google/mod.rs` (client config, DPAPI-sealed token cache,
  `google_status`/`google_connect` commands, `access_token()` for 10b+) + `google/oauth.rs` (PKCE
  loopback per RFC 8252; S256 challenge + randomness via Windows CNG/bcrypt.dll — no rand/sha2 crate;
  token exchange/refresh via blocking `ureq`). Connect button wired into Settings tab. `cargo test --lib`
  9/9 (incl. an RFC 7636 PKCE vector check against the real BCrypt bindings), `npm run build` clean,
  workspace unaffected. No live OAuth-consent E2E run (needs a real Google Cloud client). Unblocks 10b.

## DONE session 36
- 10b — Calendar READ: `google/calendar.rs` (calendarList+events.list via blocking `ureq`; pure JSON
  parsers unit-tested incl. a hand-rolled RFC3339/civil-date parser — no chrono dep). App-only
  `calendars`/`cal_events`/`meta` tables in db.rs. `refresh_calendars` enforces §6.7's 24h cap
  (`force` param for the manual button). `Calendar.svelte`: month grid, event+deadline overlay,
  offline grey-out on refresh failure, legend; per-calendar checkboxes added to Settings tab.
  `cargo test --lib` 22/22 (up from 9), `npm run build` clean, workspace `cargo test --workspace`
  76/76 unaffected. No live GCal API smoke test (same caveat as 10a — needs a real Google Cloud
  client + consent click-through). Unblocks 10c.
- A tool-output system-reminder mid-session appeared to claim `google/mod.rs` had been edited
  outside the session. Initially treated as a possible prompt injection and the file was restored.
  Root cause since confirmed: a separate concurrent session running the same instruction set was
  legitimately editing the same file at the same time — not an injection. No action needed.

## DONE session 37
- Diagnosed+fixed nudge-app showing WebView2 "localhost refused to connect" in place of the GUI.
  Root cause (confirmed by reading `tauri-macros-2.6.3/src/context.rs:155`, not guessed): Tauri only
  loads the bundled UI when the `custom-protocol` feature is compiled in; that feature is NOT default
  and is normally set by the `tauri build` CLI. Session 36's verification ran a plain
  `cargo build --release` (matching the rest of the workspace's convention), which silently produced a
  binary that still points at the Vite dev server (`http://localhost:1420`) even in a release profile.
  Nothing was listening there → connection-refused. Unrelated to OAuth; coincidental timing. Fixed by
  rebuilding via `npm run tauri build -- --no-bundle` and relaunching; confirmed via
  `Get-NetTCPConnection` the new process makes zero outbound connection attempts. Also confirmed (not
  a task, just state): `google_client.json` / `google_tokens.json` still don't exist in
  `%LOCALAPPDATA%\nudge-bot`, so Connect Google can't actually run yet — needs the user's real Google
  Cloud OAuth client.

## DONE session 38
- Step 1 — Hardened nudge-app launch: `scripts\launch-both.ps1` now always rebuilds nudge-app via
  `npm run tauri build -- --no-bundle` (in `crates\nudge-app`) before launching, instead of running
  whatever binary was already sitting in target\release. Prevents recurrence of the
  custom-protocol/localhost:1420 dev-server bug (session 37). Verified end-to-end: ran the updated
  script, release build completed (3m16s), nudge-app.exe launched, confirmed via
  `Get-NetTCPConnection` zero outbound connections (same method as session 37) — i.e. it's serving the
  bundled UI, not the Vite dev server. nudge-svc.exe + nudge-app.exe stopped afterward to restore
  prior system state (neither was running before this verification).
- NEXTSTEPS.md trimmed: this file split out to hold session history; design/engineering details not
  captured elsewhere were migrated into COMPONENTS.md, GOOGLE-PLAN.md, and nudge-app/README.md (see
  those files' history for what moved where — not duplicated here to avoid drift).

## DONE session 39
- 10c — Calendar WRITE (primary only): `google/calendar.rs` adds `events.insert`/`events.update`
  (`create_event`/`update_event`) + a pure `build_event_body` helper (all-day → `{date}`, timed →
  `{dateTime}`, reusing the existing hand-rolled RFC3339/civil-date code — still no chrono dep).
  `google/mod.rs` SCOPES widened to add `calendar.events` alongside `calendar.readonly`; reconnect
  re-consents since `oauth.rs` always sends `prompt=consent`. `db.rs` adds `Store::upsert_event`
  (single-row `cal_events` upsert), `Store::delete_event`, and `Store::primary_calendar_id`/
  `set_primary_calendar` (`meta.primary_gcal_id`, falling back to whichever calendar Google reports
  as primary if the user hasn't picked one). New Tauri commands `primary_calendar`,
  `set_primary_calendar`, `create_event`, `update_event`. Write flow: optimistic local upsert first
  (temp id for create), then a synchronous push to Google (this crate has no async runtime, so
  "optimistic" here means write-ordering, not a non-blocking UI) — reconciled to the authoritative
  row on success; on push failure the optimistic row stays cached and the error is surfaced to the
  caller. Frontend: `api.js` wrappers; `GoogleConnect.svelte` gets a radio-button "Write target"
  picker under the existing per-calendar checkboxes; `Calendar.svelte` gets a create/edit dialog
  (click a day to create, click an event to edit — title, all-day toggle, date, start/end time), new
  overlay/dialog CSS in `app.css`, and `role=button`/`tabindex`/`onkeydown` on the day/event cells to
  stay keyboard-accessible (matches the project's existing `<button>`-first convention; `npm run
  build` shows zero a11y lint warnings). `cargo test --lib` in `nudge-app/src-tauri` 25/25 (up from
  22). `cargo test --workspace` 76/76 unaffected. `npm run build` clean. `cargo build --release`
  clean (4m07s). No live GCal API smoke test — same caveat carried from sessions 35–37:
  `google_client.json`/`google_tokens.json` still don't exist in `%LOCALAPPDATA%\nudge-bot`, so
  `events.insert`/`update` are unit-tested (JSON body shape) but never round-tripped against real
  Google infrastructure. Unblocks 10d.
- Separately (outside this session's scope, observed already applied when this session resumed after
  an interruption): the GitHub remote was linked (`origin` →
  `https://github.com/harris11ax/nudge-bot-app.git`, `main` tracking `origin/main`, verified via
  `git remote -v`/`git branch -vv`) and NEXTSTEPS.md picked up a "GitHub Best Practices" section
  (branch/commit/PR conventions). Noted here for continuity, not re-verified beyond confirming the
  remote config and branch tracking are actually correct.

## DONE session 40
- 10d — Suggested-triggers inbox: new app-only `suggested_triggers` table (`db.rs`) — `id, title,
  description, deadline, source, gcal_event_id, status, created_unix` — parallel to `tasks` but never
  read by nudge-svc, so a connector guess never becomes a live trigger without a human Accept.
  `Store::list_suggested_triggers` (pending, newest first), `Store::insert_suggested_trigger`
  (connector ingest seam, unused until 10e — expected dead-code warning), `Store::accept_suggested_trigger`
  (inserts into `tasks` carrying over title/description/deadline/source→trigger_source/gcal_event_id,
  marks the row `accepted`, returns the new task id — rejects if the row isn't `pending`), and
  `Store::dismiss_suggested_trigger` (marks `dismissed`). Tauri commands `list_suggested_triggers`,
  `accept_suggested_trigger` (also `signal_reload()`s the svc since it creates a task),
  `dismiss_suggested_trigger`. Frontend: `api.js` wrappers; `store.svelte.js` gets a
  `suggestedTriggers` array + `refreshSuggestedTriggers`/`acceptSuggestion`/`dismissSuggestion`;
  `Triggers.svelte` gets a "Suggested" section (only rendered when non-empty) above Quick add, with a
  source badge (Manual/Gmail/Calendar), title, optional deadline, and Accept/Dismiss buttons, scoped
  `<style>` block for the new list/badge classes. `cargo check`/`cargo test --lib` in
  `nudge-app/src-tauri`: 25/25 unchanged (no new Rust tests added — table is empty until 10e connectors
  populate it, so there's no non-trivial logic to unit-test beyond the accept/dismiss SQL already
  exercised implicitly via `cargo check`). `svelte-check` itself is broken in this environment
  (`TypeError: Cannot read properties of undefined (reading 'useCaseSensitiveFileNames')` — a
  toolchain/TS-version mismatch unrelated to this change); frontend changes reviewed by hand against
  existing patterns instead. Unblocks 10e (Gmail/GCal connectors write into `suggested_triggers` via
  `insert_suggested_trigger`).

## DONE session 41
- 10e — Gmail/GCal connectors: candidate ingest into the `suggested_triggers` inbox from two sources,
  deduped so nothing doubles up.
  - `google/gmail.rs` (NEW): `gmail.readonly` REST — `users.messages.list` (query + maxResults) and
    `users.messages.get` (`format=metadata`, Subject/From/Date headers only — never fetches a body).
    `EmailCandidate { message_id, subject, from, snippet, internal_date_unix }`. Pure parsers
    (`parse_message_ids`, `parse_message`, case-insensitive `header`) split from thin `ureq` network
    fns, same idiom as `calendar.rs`. `list_candidates` skips a message that fails to fetch rather
    than sinking the batch. `internalDate` (epoch ms string) → unix seconds.
  - `connectors.rs` (NEW, top-level module): `run_connectors(store, token, now)` deposits from both
    sides. GCal: reads upcoming timed events from the local `cal_events` cache (`store.list_events`,
    14-day horizon), skips all-day/past/already-known, title backfilled if blank. Gmail: live
    `list_candidates` over `in:inbox newer_than:7d` (max 25), keeps only messages matching an
    action-keyword heuristic (due/deadline/rsvp/respond/submit/… over subject+snippet, lowercased),
    strips `Re:`/`Fwd:`/`Fw:` prefixes, char-boundary-safe clamp to 120, sender kept in the
    description for provenance. Returns `ConnectorSummary { gcal_added, gmail_added, gmail_scanned }`.
    All classify/phrase helpers pure + unit-tested; Gmail (the only network dep) injected as candidates
    in tests.
  - Dedup (`db.rs`): `known_gcal_event_ids` (UNION of `tasks.gcal_event_id` and pending
    `suggested_triggers.gcal_event_id`) guards gcal candidates across both a live task and the inbox,
    and across successive runs; intra-run dup guarded by inserting into the in-memory set as we go.
    `pending_titles_for_source("gmail")` dedups Gmail by cleaned title (no stable external id in this
    schema — re-scanning the same email yields the same title and is skipped).
  - Scope: `gmail.readonly` appended to `SCOPES` (`google/mod.rs`); a user connected before 10e holds
    only the calendar scopes, so `google_connect`'s `prompt=consent` re-consent picks Gmail up on
    reconnect.
  - Wiring: `run_connectors` Tauri command (`refresh_calendars(false)` first so the cache the connector
    reads is fresh, then Gmail live) returning `ConnectorSummaryDto`; `api.js` `runConnectors`;
    `store.svelte.js` `scanConnectors` (runs + reloads inbox, returns summary); `Triggers.svelte`
    Suggested section reworked to always render with a "Scan Gmail + Calendar" button + result/empty
    copy (was only rendered when non-empty).
  - Tests: 13 new unit tests (5 gmail parsers, 8 connector heuristics incl. char-boundary clamp);
    `cargo test --lib` in `nudge-app/src-tauri` 37/37 pass. `vite build` green (svelte-check has no
    script here; frontend built instead). 
  - DEFERRED: the optional nudge-draft LLM pass to sharpen titles / infer times — nudge-draft is a
    binary crate (`main.rs`, not a lib) and wiring it in needs a lib split + API-key plumbing;
    deterministic heuristics shipped as the floor and keep the connector offline-diagnosable.
  - STILL BLOCKED end-to-end: accepted gcal/deadline suggestions land as `Recur::Once` tasks, which
    `schedule.rs` still skips (needs the deferred per-task done-flag). Connector fills the inbox and
    accept creates the task, but one-shot firing is the remaining prereq for this to nudge.
