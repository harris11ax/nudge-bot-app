//! nudge-app backend (Tauri v2). Thin command layer over [`db`]: the GUI reads
//! and writes the shared `tasks` table, then pings the resident svc to reload.
//! All heavy logic (quick-add grammar, recur specs, mode labels) lives in
//! nudge-core / [`db`] so this file stays a serialization + wiring seam.

mod aw_usage;
mod connectors;
mod csv_import;
mod db;
mod google;
#[cfg(windows)]
mod ipc;

use db::{CalendarRow, EventRow, Store, SuggestedTriggerRow};
use google::calendar::EventInfo;
use nudge_core::tasks::{parse_quickadd, Recur, Task, TriggerSource};
use nudge_core::Mode;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Wire shape of a task for the Svelte frontend. `nudge_core::Task` is a pure
/// domain type with no serde derive; this DTO is the JSON contract and keeps the
/// enum encodings explicit (`recur` as its spec string, `mode`/`source` as labels).
#[derive(Serialize, Deserialize)]
pub struct TaskDto {
    pub id: Option<i64>,
    pub title: String,
    pub desc: String,
    pub deadline: Option<i64>,
    pub task_type: String,
    /// Minutes since local midnight, or null for a deadline-only task.
    pub minutes: Option<u32>,
    /// Recurrence spec (`"once"` / `"mon,wed,fri"`).
    pub recur: String,
    /// `"off_task"` / `"on_task"` / null (classify at the edge).
    pub mode_override: Option<String>,
    pub task_source: String,
    pub gcal_event_id: Option<String>,
    /// Estimated minutes (§6.3), or null for no estimate.
    pub estimate_minutes: Option<u32>,
    /// Minutes worked so far — svc-owned cache, read-only for the progress bar.
    pub logged_minutes: u32,
}

impl From<Task> for TaskDto {
    fn from(t: Task) -> Self {
        TaskDto {
            id: t.id,
            title: t.title,
            desc: t.desc,
            deadline: t.deadline,
            task_type: t.task_type,
            minutes: t.minutes,
            recur: t.recur.to_spec(),
            mode_override: match t.mode_override {
                Some(Mode::OffTask) => Some("off_task".into()),
                Some(Mode::OnTask) => Some("on_task".into()),
                None => None,
            },
            task_source: t.task_source.label().to_string(),
            gcal_event_id: t.gcal_event_id,
            estimate_minutes: t.estimate_minutes,
            logged_minutes: t.logged_minutes,
        }
    }
}

fn open() -> Result<Store, String> {
    Store::open().map_err(|e| format!("open db: {e}"))
}

/// Return every task, ordered by id.
#[tauri::command]
fn list_tasks() -> Result<Vec<TaskDto>, String> {
    let store = open()?;
    let tasks = store.list().map_err(|e| format!("list tasks: {e}"))?;
    Ok(tasks.into_iter().map(TaskDto::from).collect())
}

/// Parse a quick-add line (`text @ time [recur]`), insert it, ping the svc, and
/// return the stored task. Parse errors surface the message-bearing
/// [`nudge_core::tasks::QuickAddError`] verbatim for inline UI display.
#[tauri::command]
fn add_quickadd(line: String) -> Result<TaskDto, String> {
    let q = parse_quickadd(&line).map_err(|e| e.to_string())?;
    let task = Task {
        id: None,
        title: q.text,
        desc: String::new(),
        deadline: None,
        task_type: String::new(),
        minutes: Some(q.minutes),
        recur: q.recur,
        mode_override: None,
        task_source: TriggerSource::Manual,
        gcal_event_id: None,
        estimate_minutes: None,
        logged_minutes: 0,
    };
    insert_and_reload(task)
}

/// Fallback structured form (Triggers tab). `recur` accepts the same grammar as
/// quick-add (`once` / keyword / day list). `minutes` is optional (deadline-only
/// tasks). `mode_override` is `"off_task"`, `"on_task"`, or absent.
#[derive(Deserialize)]
pub struct NewTaskForm {
    pub title: String,
    #[serde(default)]
    pub desc: String,
    pub deadline: Option<i64>,
    #[serde(default)]
    pub task_type: String,
    pub minutes: Option<u32>,
    #[serde(default = "once_spec")]
    pub recur: String,
    pub mode_override: Option<String>,
    /// Estimated minutes (§6.3), optional.
    pub estimate_minutes: Option<u32>,
}

fn once_spec() -> String {
    "once".to_string()
}

#[tauri::command]
fn add_task(form: NewTaskForm) -> Result<TaskDto, String> {
    let title = form.title.trim();
    if title.is_empty() {
        return Err("title is empty".into());
    }
    let recur = Recur::parse(&form.recur).map_err(|e| e.to_string())?;
    let mode_override = match form.mode_override.as_deref() {
        Some("off_task") => Some(Mode::OffTask),
        Some("on_task") => Some(Mode::OnTask),
        _ => None,
    };
    let task = Task {
        id: None,
        title: title.to_string(),
        desc: form.desc,
        deadline: form.deadline,
        task_type: form.task_type,
        minutes: form.minutes,
        recur,
        mode_override,
        task_source: TriggerSource::Manual,
        gcal_event_id: None,
        estimate_minutes: form.estimate_minutes,
        logged_minutes: 0,
    };
    insert_and_reload(task)
}

/// Task id passed via `--task <id>` on a cold-start launch (9d-ii click-through).
/// `Some` exactly once: the frontend takes it on first mount and it's consumed
/// after that, so a later window refresh doesn't re-open the same task.
struct PendingTask(std::sync::Mutex<Option<i64>>);

/// One-shot pull of the cold-start `--task` id, if any. `None` on a plain
/// launch, or once the frontend has already taken it.
#[tauri::command]
fn get_pending_task(state: tauri::State<PendingTask>) -> Option<i64> {
    state.0.lock().unwrap().take()
}

/// Delete a task by id, then ping the svc.
#[tauri::command]
fn delete_task(id: i64) -> Result<(), String> {
    let store = open()?;
    store.delete(id).map_err(|e| format!("delete: {e}"))?;
    db::signal_reload();
    Ok(())
}

fn insert_and_reload(mut task: Task) -> Result<TaskDto, String> {
    let store = open()?;
    let id = store.insert(&task).map_err(|e| format!("insert: {e}"))?;
    task.id = Some(id);
    db::signal_reload();
    Ok(TaskDto::from(task))
}

// --- Suggested-triggers inbox (10d) ---

/// Wire shape of a pending suggestion (Triggers tab — Suggested section).
#[derive(Serialize)]
pub struct SuggestedTriggerDto {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub deadline: Option<i64>,
    pub source: String,
    pub gcal_event_id: Option<String>,
    pub created_unix: i64,
}

impl From<SuggestedTriggerRow> for SuggestedTriggerDto {
    fn from(r: SuggestedTriggerRow) -> Self {
        SuggestedTriggerDto {
            id: r.id,
            title: r.title,
            description: r.description,
            deadline: r.deadline,
            source: r.source,
            gcal_event_id: r.gcal_event_id,
            created_unix: r.created_unix,
        }
    }
}

/// Pending suggestions, newest first.
#[tauri::command]
fn list_suggested_triggers() -> Result<Vec<SuggestedTriggerDto>, String> {
    let store = open()?;
    let rows = store
        .list_suggested_triggers()
        .map_err(|e| format!("list suggested triggers: {e}"))?;
    Ok(rows.into_iter().map(SuggestedTriggerDto::from).collect())
}

/// Accept a suggestion: creates the live task and pings the svc, returning the
/// new task's id.
#[tauri::command]
fn accept_suggested_trigger(id: i64) -> Result<i64, String> {
    let store = open()?;
    let task_id = store
        .accept_suggested_trigger(id)
        .map_err(|e| format!("accept suggested trigger: {e}"))?;
    db::signal_reload();
    Ok(task_id)
}

/// Dismiss a suggestion; no task is created.
#[tauri::command]
fn dismiss_suggested_trigger(id: i64) -> Result<(), String> {
    let store = open()?;
    store
        .dismiss_suggested_trigger(id)
        .map_err(|e| format!("dismiss suggested trigger: {e}"))?;
    Ok(())
}

// --- Calendar (10b, read-only) ---

/// Wire shape of a calendar for the Svelte frontend (Settings checkboxes +
/// Calendar tab legend).
#[derive(Serialize)]
pub struct CalendarDto {
    pub gcal_id: String,
    pub summary: String,
    pub bg_color: String,
    pub selected: bool,
    pub is_primary: bool,
}

impl From<CalendarRow> for CalendarDto {
    fn from(c: CalendarRow) -> Self {
        CalendarDto {
            gcal_id: c.gcal_id,
            summary: c.summary,
            bg_color: c.bg_color,
            selected: c.selected,
            is_primary: c.is_primary,
        }
    }
}

/// Wire shape of a cached event for the Calendar tab's month/week grid.
#[derive(Serialize)]
pub struct EventDto {
    pub event_id: String,
    pub calendar_id: String,
    pub summary: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub all_day: bool,
}

impl From<EventRow> for EventDto {
    fn from(e: EventRow) -> Self {
        EventDto {
            event_id: e.event_id,
            calendar_id: e.calendar_id,
            summary: e.summary,
            start_unix: e.start_unix,
            end_unix: e.end_unix,
            all_day: e.all_day,
        }
    }
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_secs() as i64
}

/// Calendars known locally (cached overlay/selection state) — works offline.
#[tauri::command]
fn list_calendars() -> Result<Vec<CalendarDto>, String> {
    let store = open()?;
    let rows = store
        .list_calendars()
        .map_err(|e| format!("list calendars: {e}"))?;
    Ok(rows.into_iter().map(CalendarDto::from).collect())
}

/// Toggle a calendar's overlay checkbox (Settings — Calendar).
#[tauri::command]
fn set_calendar_selected(gcal_id: String, selected: bool) -> Result<(), String> {
    let store = open()?;
    store
        .set_calendar_selected(&gcal_id, selected)
        .map_err(|e| format!("set calendar selected: {e}"))
}

/// §6.7 refresh policy: launch / every 24h since the last success / manual
/// button. `force` (the Refresh button) bypasses the 24h cap; the frontend's
/// launch-time call passes `force=false` and this is a no-op if still fresh.
/// Returns the resulting `google_last_refresh` unix timestamp (unchanged if
/// this call was a cap no-op). A network/auth failure leaves the cache intact
/// and returns `Err` — callers must keep rendering cached events on failure
/// (GOOGLE-PLAN.md constraint #4: offline degrades, never breaks).
#[tauri::command]
fn refresh_calendars(force: bool) -> Result<i64, String> {
    let mut store = open()?;
    let now = now_unix();
    if !force {
        if let Some(last_ts) = store
            .get_meta("google_last_refresh")
            .map_err(|e| format!("get meta: {e}"))?
            .and_then(|s| s.parse::<i64>().ok())
        {
            if now - last_ts < 24 * 3600 {
                return Ok(last_ts);
            }
        }
    }

    let token = google::access_token()?;
    let cals = google::calendar::list_calendars(&token)?;
    store
        .upsert_calendars(&cals)
        .map_err(|e| format!("upsert calendars: {e}"))?;

    // Only pull events for selected calendars — keeps request count down and
    // matches "deselected calendars grey out" (no reason to cache their events).
    let known = store
        .list_calendars()
        .map_err(|e| format!("list calendars: {e}"))?;
    let window_start = now - 7 * 24 * 3600;
    let window_end = now + 60 * 24 * 3600;
    for cal in known.iter().filter(|c| c.selected) {
        let events = google::calendar::list_events(&token, &cal.gcal_id, window_start, window_end)?;
        store
            .replace_events(&cal.gcal_id, &events)
            .map_err(|e| format!("replace events for {}: {e}", cal.gcal_id))?;
    }

    store
        .set_meta("google_last_refresh", &now.to_string())
        .map_err(|e| format!("set meta: {e}"))?;
    Ok(now)
}

/// Cached events overlapping `[from, to)` unix seconds — the offline-safe read
/// path the Calendar tab renders from regardless of `refresh_calendars` result.
#[tauri::command]
fn list_events(from: i64, to: i64) -> Result<Vec<EventDto>, String> {
    let store = open()?;
    let rows = store
        .list_events(from, to)
        .map_err(|e| format!("list events: {e}"))?;
    Ok(rows.into_iter().map(EventDto::from).collect())
}

/// Last successful refresh (unix seconds), or `None` if never refreshed —
/// drives the "last refreshed" stamp next to the Calendar tab's Refresh button.
#[tauri::command]
fn google_last_refresh() -> Result<Option<i64>, String> {
    let store = open()?;
    Ok(store
        .get_meta("google_last_refresh")
        .map_err(|e| format!("get meta: {e}"))?
        .and_then(|s| s.parse().ok()))
}

/// Connector run result surfaced to the Triggers tab as a toast.
#[derive(Serialize)]
pub struct ConnectorSummaryDto {
    pub gcal_added: usize,
    pub gmail_added: usize,
    pub gmail_scanned: usize,
}

/// Run the Gmail/GCal connectors (10e): scan upcoming calendar events + recent
/// actionable mail and deposit deduped candidates into the Suggested-triggers
/// inbox. Piggybacks a refresh so the calendar cache is fresh first; a Gmail
/// failure surfaces as `Err` but any calendar suggestions already deposited
/// persist. The frontend refreshes the Suggested section on success.
#[tauri::command]
fn run_connectors() -> Result<ConnectorSummaryDto, String> {
    // Ensure the calendar cache the connector reads from is current.
    refresh_calendars(false)?;
    let store = open()?;
    let token = google::access_token()?;
    let s = connectors::run_connectors(&store, &token, now_unix())?;
    Ok(ConnectorSummaryDto {
        gcal_added: s.gcal_added,
        gmail_added: s.gmail_added,
        gmail_scanned: s.gmail_scanned,
    })
}

// --- Calendar (10c, write — primary calendar only) ---

/// The app-chosen write-target calendar id, or `None` if not yet picked (and
/// Google hasn't reported one as primary either — e.g. before first refresh).
#[tauri::command]
fn primary_calendar() -> Result<Option<String>, String> {
    let store = open()?;
    store
        .primary_calendar_id()
        .map_err(|e| format!("primary calendar: {e}"))
}

/// Set the write-target calendar (Settings — Calendar picker).
#[tauri::command]
fn set_primary_calendar(gcal_id: String) -> Result<(), String> {
    let store = open()?;
    store
        .set_primary_calendar(&gcal_id)
        .map_err(|e| format!("set primary calendar: {e}"))
}

/// Create/edit dialog payload — unix-second bounds already resolved by the
/// frontend (same convention as [`EventDto`]/[`EventRow`]).
#[derive(Deserialize)]
pub struct EventForm {
    pub summary: String,
    pub start_unix: i64,
    pub end_unix: i64,
    #[serde(default)]
    pub all_day: bool,
}

fn require_primary_calendar(store: &Store) -> Result<String, String> {
    store
        .primary_calendar_id()
        .map_err(|e| format!("primary calendar: {e}"))?
        .ok_or_else(|| "no primary calendar set — pick one in Settings first".to_string())
}

/// Create an event on the primary calendar. Writes an optimistic local row
/// under a temporary id first (so a push failure still leaves *something*
/// visible offline, per GOOGLE-PLAN.md constraint #4), then pushes to Google
/// synchronously — this command's architecture is blocking end-to-end like
/// every other Google call here (no async runtime in this crate), so
/// "optimistic" describes write ordering, not a non-blocking UI: on success the
/// temp row is swapped for the authoritative one; on failure the temp row is
/// left in place and the error is surfaced to the caller.
#[tauri::command]
fn create_event(form: EventForm) -> Result<EventDto, String> {
    let store = open()?;
    let calendar_id = require_primary_calendar(&store)?;

    let temp_id = format!("local-{}-{}", now_unix(), std::process::id());
    let optimistic = EventInfo {
        event_id: temp_id.clone(),
        summary: form.summary.clone(),
        start_unix: form.start_unix,
        end_unix: form.end_unix,
        all_day: form.all_day,
        updated_unix: now_unix(),
        etag: String::new(),
    };
    store
        .upsert_event(&calendar_id, &optimistic)
        .map_err(|e| format!("optimistic insert: {e}"))?;

    let token = google::access_token()?;
    let pushed = google::calendar::create_event(
        &token,
        &calendar_id,
        &form.summary,
        form.start_unix,
        form.end_unix,
        form.all_day,
    );
    match pushed {
        Ok(authoritative) => {
            store
                .delete_event(&temp_id)
                .map_err(|e| format!("drop temp event: {e}"))?;
            store
                .upsert_event(&calendar_id, &authoritative)
                .map_err(|e| format!("cache created event: {e}"))?;
            Ok(EventDto {
                event_id: authoritative.event_id,
                calendar_id,
                summary: authoritative.summary,
                start_unix: authoritative.start_unix,
                end_unix: authoritative.end_unix,
                all_day: authoritative.all_day,
            })
        }
        Err(e) => Err(format!(
            "saved locally, but push to Google failed (will retry next refresh): {e}"
        )),
    }
}

/// Update an existing event's summary/time on the primary calendar. Same
/// optimistic-then-push shape as [`create_event`], but the id is already known
/// so there's no temp-row swap — a failed push just leaves the optimistic
/// (new) values cached locally alongside the surfaced error.
#[tauri::command]
fn update_event(event_id: String, form: EventForm) -> Result<EventDto, String> {
    let store = open()?;
    let calendar_id = require_primary_calendar(&store)?;

    let optimistic = EventInfo {
        event_id: event_id.clone(),
        summary: form.summary.clone(),
        start_unix: form.start_unix,
        end_unix: form.end_unix,
        all_day: form.all_day,
        updated_unix: now_unix(),
        etag: String::new(),
    };
    store
        .upsert_event(&calendar_id, &optimistic)
        .map_err(|e| format!("optimistic update: {e}"))?;

    let token = google::access_token()?;
    let pushed = google::calendar::update_event(
        &token,
        &calendar_id,
        &event_id,
        &form.summary,
        form.start_unix,
        form.end_unix,
        form.all_day,
    );
    match pushed {
        Ok(authoritative) => {
            store
                .upsert_event(&calendar_id, &authoritative)
                .map_err(|e| format!("cache updated event: {e}"))?;
            Ok(EventDto {
                event_id: authoritative.event_id,
                calendar_id,
                summary: authoritative.summary,
                start_unix: authoritative.start_unix,
                end_unix: authoritative.end_unix,
                all_day: authoritative.all_day,
            })
        }
        Err(e) => Err(format!(
            "saved locally, but push to Google failed (will retry next refresh): {e}"
        )),
    }
}

// --- Task tools + app classification (§6.2, PLAN-step3 P3) ---

/// Wire shape of a selector/Settings app row (`app_usage` ⟕ `app_classes`).
#[derive(Serialize)]
pub struct AppDto {
    pub name: String,
    pub minutes_90d: i64,
    /// `favorite | normal | hidden | not_tool` (unclassified reads `normal`).
    pub class: String,
}

impl From<db::AppRow> for AppDto {
    fn from(a: db::AppRow) -> Self {
        AppDto { name: a.name, minutes_90d: a.minutes_90d, class: a.class }
    }
}

/// One `(app_name, kind)` tool row; `kind ∈ tool | ignore`.
#[derive(Serialize, Deserialize)]
pub struct ToolDto {
    pub app_name: String,
    pub kind: String,
}

/// Every app the Tools selector can offer, usage-sorted descending. The
/// frontend pins favorites and filters hidden — this is the raw union.
#[tauri::command]
fn list_apps_for_selector() -> Result<Vec<AppDto>, String> {
    let store = open()?;
    let rows = store
        .list_apps_for_selector()
        .map_err(|e| format!("list apps: {e}"))?;
    Ok(rows.into_iter().map(AppDto::from).collect())
}

/// Replace a task's tool list wholesale (§6.4 contract: svc reads this at the
/// sample compare), then ping the svc so the change is live immediately.
#[tauri::command]
fn set_task_tools_cmd(task_id: i64, tools: Vec<ToolDto>) -> Result<(), String> {
    let mut store = open()?;
    let pairs: Vec<(String, String)> = tools.into_iter().map(|t| (t.app_name, t.kind)).collect();
    store
        .set_task_tools(task_id, &pairs)
        .map_err(|e| format!("set task tools: {e}"))?;
    db::signal_reload();
    Ok(())
}

/// A task's `(app_name, kind)` tool rows.
#[tauri::command]
fn list_task_tools_cmd(task_id: i64) -> Result<Vec<ToolDto>, String> {
    let store = open()?;
    let rows = store
        .list_task_tools(task_id)
        .map_err(|e| format!("list task tools: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(app_name, kind)| ToolDto { app_name, kind })
        .collect())
}

/// Set an app's global class (§6.2 favorite|normal|hidden|not_tool).
#[tauri::command]
fn set_app_class_cmd(app_name: String, class: String) -> Result<(), String> {
    if !matches!(class.as_str(), "favorite" | "normal" | "hidden" | "not_tool") {
        return Err(format!("unknown app class: {class}"));
    }
    let store = open()?;
    store
        .set_app_class(&app_name, &class)
        .map_err(|e| format!("set app class: {e}"))
}

/// Explicitly-classified apps (Settings — Tools tab).
#[tauri::command]
fn list_app_classes() -> Result<Vec<AppDto>, String> {
    let store = open()?;
    let rows = store
        .list_app_classes()
        .map_err(|e| format!("list app classes: {e}"))?;
    Ok(rows.into_iter().map(AppDto::from).collect())
}

/// Not-Tool recommendation seed: high-usage apps never used as any task's tool
/// and not yet classified.
#[tauri::command]
fn list_not_tool_candidates() -> Result<Vec<AppDto>, String> {
    let store = open()?;
    let rows = store
        .list_not_tool_candidates(20)
        .map_err(|e| format!("list not-tool candidates: {e}"))?;
    Ok(rows.into_iter().map(AppDto::from).collect())
}

/// Read-only view of per-task ignore rows (Settings — Tools tab).
#[derive(Serialize)]
pub struct IgnoreDto {
    pub task_id: i64,
    pub task_title: String,
    pub app_name: String,
}

#[tauri::command]
fn list_task_ignores() -> Result<Vec<IgnoreDto>, String> {
    let store = open()?;
    let rows = store
        .list_all_ignores()
        .map_err(|e| format!("list ignores: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(task_id, task_title, app_name)| IgnoreDto { task_id, task_title, app_name })
        .collect())
}

/// §6.9 Style tab: completion-band colors persisted as a JSON array in `meta`.
/// Empty vec = defaults (renderers keep their built-in palette).
#[tauri::command]
fn get_style_bands() -> Result<Vec<String>, String> {
    let store = open()?;
    let raw = store
        .get_meta("style_bands")
        .map_err(|e| format!("get style bands: {e}"))?;
    match raw {
        Some(s) => serde_json::from_str(&s).map_err(|e| format!("parse style bands: {e}")),
        None => Ok(Vec::new()),
    }
}

#[tauri::command]
fn set_style_bands(bands: Vec<String>) -> Result<(), String> {
    let store = open()?;
    let s = serde_json::to_string(&bands).map_err(|e| format!("encode style bands: {e}"))?;
    store
        .set_meta("style_bands", &s)
        .map_err(|e| format!("set style bands: {e}"))
}

/// Refresh the `app_usage` cache from ActivityWatch (90-day window aggregate),
/// respecting the §6.7 24h cap unless `force`. Returns the resulting
/// `app_usage_last_refresh` unix stamp (unchanged on a cap no-op). AW being
/// down is not an error — it just refreshes zero rows and does NOT stamp, so
/// the next call retries.
#[tauri::command]
fn refresh_app_usage(force: bool) -> Result<i64, String> {
    let store = open()?;
    let now = now_unix();
    if !force {
        if let Some(last_ts) = store
            .get_meta("app_usage_last_refresh")
            .map_err(|e| format!("get meta: {e}"))?
            .and_then(|s| s.parse::<i64>().ok())
        {
            if now - last_ts < 24 * 3600 {
                return Ok(last_ts);
            }
        }
    }
    let usage = aw_usage::fetch_usage(90, now);
    if usage.is_empty() {
        // AW down or no window bucket: keep the stale cache + stamp so a
        // retry isn't gated behind the 24h cap.
        return Ok(store
            .get_meta("app_usage_last_refresh")
            .map_err(|e| format!("get meta: {e}"))?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0));
    }
    for (app, minutes) in &usage {
        store
            .upsert_app_usage(app, *minutes, now)
            .map_err(|e| format!("upsert app usage: {e}"))?;
    }
    store
        .set_meta("app_usage_last_refresh", &now.to_string())
        .map_err(|e| format!("set meta: {e}"))?;
    Ok(now)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(windows)]
    let pending_task = ipc::cli_task_id();
    #[cfg(not(windows))]
    let pending_task: Option<i64> = None;

    tauri::Builder::default()
        .manage(PendingTask(std::sync::Mutex::new(pending_task)))
        .plugin(tauri_plugin_opener::init())
        .setup(|_app| {
            #[cfg(windows)]
            ipc::install_copydata_handler(_app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_tasks,
            add_quickadd,
            add_task,
            delete_task,
            get_pending_task,
            google::google_status,
            google::google_connect,
            google::google_disconnect,
            list_calendars,
            set_calendar_selected,
            refresh_calendars,
            list_events,
            google_last_refresh,
            primary_calendar,
            set_primary_calendar,
            create_event,
            update_event,
            list_suggested_triggers,
            accept_suggested_trigger,
            dismiss_suggested_trigger,
            run_connectors,
            list_apps_for_selector,
            set_task_tools_cmd,
            list_task_tools_cmd,
            set_app_class_cmd,
            list_app_classes,
            list_not_tool_candidates,
            list_task_ignores,
            get_style_bands,
            set_style_bands,
            refresh_app_usage
        ])
        .run(tauri::generate_context!())
        .expect("error while running nudge-app");
}
