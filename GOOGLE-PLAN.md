# GOOGLE-PLAN.md — §10 Calendar + Connectors design (Opus planning, session 34)
<!-- Boundary: design only. ALL Google network I/O lives in nudge-app; nudge-svc stays offline/zero-poll. -->

## Scope (UI-PLAN P2–P4)
Multi-Google-Calendar **read** overlay, **write** to one primary calendar, **Suggested-triggers**
inbox, Gmail/GCal **connectors**. Builds on existing scaffolding already in tree:
`TriggerSource::{Gmail,Gcal}`, `Task.gcal_event_id`, `tasks.gcal_event_id` column.

## Load-bearing constraints
1. **svc never touches network.** All OAuth/HTTP is in `nudge-app` backend (own process, own budget,
   UI-PLAN §0). svc only ever sees rows in `tasks` it already reads.
2. **Reuse the nudge-draft HTTP idiom:** blocking `ureq` + `serde_json`, no async (workspace convention,
   see `nudge-draft/src/anthropic.rs`). No new async runtime in nudge-app backend.
3. **First-writer-wins schema parity:** any new table the svc must read gets identical
   `CREATE TABLE IF NOT EXISTS` DDL in BOTH `nudge-app/src/db.rs` and `nudge-svc/src/persist.rs`
   (existing `tasks` contract). Tables only nudge-app reads → app-side only.
4. **Offline degrades, never breaks:** Calendar tab renders cached events + task deadlines with no
   network; Google layers grey out. Refresh cadence capped per §6.7 (launch / 24 h / manual button).
5. **Credentials never in chat/code.** OAuth is the end-user's own browser consent (PKCE loopback).
   No secret hardcoded; installed-app client id via config. Tokens cached locally, never logged.

## Module layout (new, all under `crates/nudge-app/src-tauri/src/`)
```
google/mod.rs        connect state, token cache load/save, shared ureq agent + auth header
google/oauth.rs      OAuth 2.0 PKCE, loopback redirect (127.0.0.1:<ephemeral>), refresh
google/calendar.rs   GCal REST: calendarList, events.list, events.insert/update (primary only)
google/gmail.rs      (P4) gmail.readonly: messages.list/get → candidate extraction
connectors.rs        (P4) event/email → suggested_triggers dedup + deposit
```
Pure parse/dedup helpers unit-tested (mirror anthropic.rs `build_body`/`extract_text` split);
network fns are thin and integration-tested manually.

**Implemented (10b):** `calendar.rs`'s JSON parsers include a hand-rolled RFC3339/civil-date parser
instead of a `chrono` dependency — same minimal-deps rationale as the PKCE choice above. Keep this
convention for any future date/time parsing added to nudge-app; don't reach for chrono without a
reason strong enough to revisit the convention deliberately.

**Not yet done:** no live OAuth-consent or live GCal API smoke test has been run end-to-end — both
10a and 10b were verified via unit tests only. Needs the user's real Google Cloud OAuth client
(`google_client.json`) before a first live run; `google_client.json`/`google_tokens.json` do not yet
exist in `%LOCALAPPDATA%\nudge-bot`.

## OAuth (installed-app, PKCE)
- Flow: open system browser to Google consent → loopback `http://127.0.0.1:<port>` catches the code →
  exchange for access+refresh token. Tiny one-shot `TcpListener` in oauth.rs (std only).
- Scopes, added per phase: `calendar.readonly` (P2), `calendar.events` (P3), `gmail.readonly` (P4).
- Client id: read from `%LOCALAPPDATA%\nudge-bot\google_client.json` (user creates a Desktop-app
  OAuth client in Google Cloud; documented in README). Absent → Calendar tab shows "Connect Google".
- Token cache: `%LOCALAPPDATA%\nudge-bot\google_tokens.json` (refresh_token + expiry). Note for impl:
  wrap with Windows DPAPI (`CryptProtectData`) before disk — refresh_token is long-lived.
- **Implemented (10a):** S256 PKCE challenge + verifier randomness use Windows CNG (`bcrypt.dll`)
  directly rather than pulling in `rand`/`sha2` crates — keeps the dependency tree matching the
  workspace's minimal-deps convention. Verified against a real RFC 7636 PKCE test vector run through
  the actual BCrypt bindings (not a mocked hash), see `google/oauth.rs` unit tests.
- Policy note: OAuth consent is a user-in-chat gated action if Claude ever drives it, but here it is
  the END USER clicking through their own browser — nudge-app only opens the URL.

## DB additions (app-writer; svc reads only `tasks`)
- `calendars(id, gcal_id UNIQUE, summary, bg_color, selected INT, is_primary INT)` — persisted overlay
  selection + which one is the write target. App-only.
- `cal_events(event_id PRIMARY KEY, calendar_id, summary, start_unix, end_unix, all_day INT,
  updated_unix, etag)` — render cache so the tab works offline. App-only. Cleared+repopulated per refresh.
- `suggested_triggers(id, source, title, minutes, deadline, gcal_event_id, raw_json, created_unix,
  status TEXT DEFAULT 'pending')` — inbox. App-only. Schema goes LIVE in P3 (empty), populated in P4.
- `meta(key, value)` kv (if not present): `google_last_refresh`, `primary_gcal_id`, connect state.

## Tauri commands (backend surface)
P2: `google_status` · `google_connect` · `list_calendars` · `set_calendar_selected(gcal_id, bool)` ·
    `refresh_calendars()` (respects §6.7 cap; returns last_refresh) · `list_events(from,to)` (cache read).
P3: `set_primary_calendar(gcal_id)` · `create_event(form)` · `update_event(id, form)` (primary only,
    optimistic local write then push) ·
    inbox: `list_suggested()` · `accept_suggested(id)` (→ insert `tasks` + `signal_reload`) ·
    `dismiss_suggested(id)`.
P4: `run_connectors()` (piggybacks refresh): GCal events + Gmail candidates → dedup vs tasks.gcal_event_id
    and pending suggestions → deposit. Optional nudge-draft pass to phrase title / infer time.

## Frontend (Svelte tabs)
- **Calendar tab (P2/P3):** month+week grid; overlays (a) selected-calendar events, (b) task deadlines
  (distinct style). Top control row: month nav + **Refresh** + last-refreshed stamp (§6.7). Event
  create/edit dialog writes primary only (P3). Offline → Google layers greyed.
- **Triggers tab — Suggested section (P3):** pending suggestions with source badge; one-click Accept /
  Dismiss. Empty until P4 populates.
- **Settings — Calendar (P2/P3):** Connect/Disconnect Google, per-calendar overlay checkboxes,
  primary-calendar picker.

## Cross-cutting dependency (flag, do not silently skip)
Calendar-derived tasks are one-shot at a date → `Recur::Once`, which `schedule.rs` **currently skips**
(needs the deferred per-task done-flag; NEXTSTEPS session 33). So accepted gcal suggestions will persist
but NOT fire until once/deadline firing lands. **Prereq for P4 to be end-to-end useful.**
§6.6 calendar-linked pause (event-card pause checkbox) is §11–12 (step 2) territory — out of scope here;
`cal_events.event_id` is the local key it will reuse.

## Decomposition → NEXTSTEPS sub-steps (each its own session)
1. **10a — Google OAuth + token cache** (`google/mod.rs`+`oauth.rs`, PKCE loopback, DPAPI, `google_status`
   /`google_connect`). No UI beyond a Connect button. **Sonnet** once this plan is accepted.
2. **10b — Calendar READ** (`calendar.rs` calendarList+events.list, `calendars`/`cal_events` tables,
   refresh cap §6.7, Calendar tab month/week overlay, offline grey-out). **Sonnet.**
3. **10c — Calendar WRITE** (primary picker, events.insert/update, create/edit dialog). **Sonnet.**
4. **10d — Suggested-triggers inbox** (`suggested_triggers` table + Triggers-tab section + accept/dismiss
   → tasks + reload). **Sonnet.**
5. **10e — Connectors** (GCal events + Gmail → suggestions, dedup, optional nudge-draft). Depends on
   once/deadline firing to be useful. **Opus** (connector heuristics + LLM extraction design).
```
```
