//! nudge-app backend (Tauri v2). Thin command layer over [`db`]: the GUI reads
//! and writes the shared `tasks` table, then pings the resident svc to reload.
//! All heavy logic (quick-add grammar, recur specs, mode labels) lives in
//! nudge-core / [`db`] so this file stays a serialization + wiring seam.

mod db;
mod google;
#[cfg(windows)]
mod ipc;

use db::{CalendarRow, EventRow, Store};
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
    pub trigger_source: String,
    pub gcal_event_id: Option<String>,
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
            trigger_source: t.trigger_source.label().to_string(),
            gcal_event_id: t.gcal_event_id,
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
        trigger_source: TriggerSource::Manual,
        gcal_event_id: None,
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
        trigger_source: TriggerSource::Manual,
        gcal_event_id: None,
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
            list_calendars,
            set_calendar_selected,
            refresh_calendars,
            list_events,
            google_last_refresh
        ])
        .run(tauri::generate_context!())
        .expect("error while running nudge-app");
}
