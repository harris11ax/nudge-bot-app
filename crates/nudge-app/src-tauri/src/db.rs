//! nudge-app's write side of the app-writer / svc-reader `tasks` contract
//! (UI-PLAN §3). The GUI is the SOLE writer; the resident nudge-svc opens the
//! same `sessions.db` read-only (`persist::Db::tasks`) and re-reads on the
//! `Local\nudge-bot-reload` signal. The CREATE batch here is kept byte-compatible
//! with nudge-svc's `persist.rs` so whichever process touches the file first
//! creates an identical schema.

use crate::google::calendar::{CalendarInfo, EventInfo};
use nudge_core::tasks::{Recur, Task, TriggerSource};
use nudge_core::Mode;
use rusqlite::OptionalExtension;
use std::path::PathBuf;

/// `%LOCALAPPDATA%\nudge-bot` — the shared config/data dir, mirroring the svc's
/// `dirs_config()`. Panics if `LOCALAPPDATA` is unset (Windows always sets it).
pub fn config_dir() -> PathBuf {
    PathBuf::from(std::env::var("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("nudge-bot")
}

/// Path to the shared session/tasks database.
pub fn db_path() -> PathBuf {
    config_dir().join("sessions.db")
}

pub struct Store {
    conn: rusqlite::Connection,
}

impl Store {
    /// Open (creating the dir + schema if needed). Idempotent with the svc's own
    /// `Db::open`.
    pub fn open() -> rusqlite::Result<Self> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let conn = rusqlite::Connection::open(&path)?;
        // Byte-for-byte the `tasks` DDL from nudge-svc/src/persist.rs. Both sides
        // use CREATE TABLE IF NOT EXISTS so first-writer-wins is safe.
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE;
             CREATE TABLE IF NOT EXISTS tasks (
                 id             INTEGER PRIMARY KEY,
                 title          TEXT    NOT NULL,
                 description    TEXT    NOT NULL DEFAULT '',
                 deadline       INTEGER,
                 task_type      TEXT    NOT NULL DEFAULT '',
                 minutes        INTEGER,
                 recur          TEXT    NOT NULL DEFAULT 'once',
                 mode_override  TEXT,
                 trigger_source TEXT    NOT NULL DEFAULT 'manual',
                 gcal_event_id  TEXT
             );
             -- App-only tables (GOOGLE-PLAN.md §DB additions): the svc never reads
             -- these, so unlike `tasks` there is no parity copy in persist.rs.
             CREATE TABLE IF NOT EXISTS calendars (
                 id         INTEGER PRIMARY KEY,
                 gcal_id    TEXT    NOT NULL UNIQUE,
                 summary    TEXT    NOT NULL DEFAULT '',
                 bg_color   TEXT    NOT NULL DEFAULT '',
                 selected   INTEGER NOT NULL DEFAULT 1,
                 is_primary INTEGER NOT NULL DEFAULT 0
             );
             CREATE TABLE IF NOT EXISTS cal_events (
                 event_id     TEXT    PRIMARY KEY,
                 calendar_id  TEXT    NOT NULL,
                 summary      TEXT    NOT NULL DEFAULT '',
                 start_unix   INTEGER NOT NULL,
                 end_unix     INTEGER NOT NULL,
                 all_day      INTEGER NOT NULL DEFAULT 0,
                 updated_unix INTEGER NOT NULL DEFAULT 0,
                 etag         TEXT    NOT NULL DEFAULT ''
             );
             CREATE TABLE IF NOT EXISTS meta (
                 key   TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )?;
        Ok(Self { conn })
    }

    /// All tasks, ordered by id (stable). Tolerant read-back mirrors the svc:
    /// a malformed `recur` → [`Recur::Once`], unknown `trigger_source` → Manual.
    pub fn list(&self) -> rusqlite::Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, description, deadline, task_type, minutes,
                    recur, mode_override, trigger_source, gcal_event_id
             FROM tasks ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            let recur_spec: String = r.get(6)?;
            let mode_s: Option<String> = r.get(7)?;
            let source_s: String = r.get(8)?;
            Ok(Task {
                id: r.get(0)?,
                title: r.get(1)?,
                desc: r.get(2)?,
                deadline: r.get(3)?,
                task_type: r.get(4)?,
                minutes: r.get::<_, Option<i64>>(5)?.map(|m| m as u32),
                recur: Recur::parse(&recur_spec).unwrap_or(Recur::Once),
                mode_override: mode_s.as_deref().and_then(parse_mode),
                trigger_source: TriggerSource::from_label(&source_s),
                gcal_event_id: r.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// Insert a task, returning the assigned rowid. `id` on the input is ignored.
    pub fn insert(&self, t: &Task) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO tasks
                (title, description, deadline, task_type, minutes,
                 recur, mode_override, trigger_source, gcal_event_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                t.title,
                t.desc,
                t.deadline,
                t.task_type,
                t.minutes.map(|m| m as i64),
                t.recur.to_spec(),
                mode_label(t.mode_override),
                t.trigger_source.label(),
                t.gcal_event_id,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Delete by rowid. Returns the number of rows removed (0 if not found).
    pub fn delete(&self, id: i64) -> rusqlite::Result<usize> {
        self.conn
            .execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id])
    }

    /// All calendars, alphabetical by summary. Persisted `selected`/`is_primary`
    /// state lives here even when offline (Calendar tab's grey-out reads this).
    pub fn list_calendars(&self) -> rusqlite::Result<Vec<CalendarRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT gcal_id, summary, bg_color, selected, is_primary
             FROM calendars ORDER BY summary",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CalendarRow {
                gcal_id: r.get(0)?,
                summary: r.get(1)?,
                bg_color: r.get(2)?,
                selected: r.get::<_, i64>(3)? != 0,
                is_primary: r.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect()
    }

    /// Merge freshly-fetched calendars from Google: `summary`/`bg_color`/
    /// `is_primary` always follow Google, but a row's `selected` flag is
    /// local-only and untouched on update — a fresh row defaults to selected.
    pub fn upsert_calendars(&self, cals: &[CalendarInfo]) -> rusqlite::Result<()> {
        for c in cals {
            self.conn.execute(
                "INSERT INTO calendars (gcal_id, summary, bg_color, selected, is_primary)
                 VALUES (?1, ?2, ?3, 1, ?4)
                 ON CONFLICT(gcal_id) DO UPDATE SET
                     summary = excluded.summary,
                     bg_color = excluded.bg_color,
                     is_primary = excluded.is_primary",
                rusqlite::params![c.gcal_id, c.summary, c.bg_color, c.is_primary as i64],
            )?;
        }
        Ok(())
    }

    /// Toggle a calendar's overlay/inclusion state (Settings — Calendar checkboxes).
    pub fn set_calendar_selected(&self, gcal_id: &str, selected: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE calendars SET selected = ?1 WHERE gcal_id = ?2",
            rusqlite::params![selected as i64, gcal_id],
        )?;
        Ok(())
    }

    /// Replace the cached events for one calendar: clear + repopulate, per
    /// GOOGLE-PLAN.md's cache policy (so a deleted/moved event doesn't linger).
    pub fn replace_events(&mut self, calendar_id: &str, events: &[EventInfo]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM cal_events WHERE calendar_id = ?1",
            rusqlite::params![calendar_id],
        )?;
        for e in events {
            tx.execute(
                "INSERT INTO cal_events
                    (event_id, calendar_id, summary, start_unix, end_unix, all_day, updated_unix, etag)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                rusqlite::params![
                    e.event_id,
                    calendar_id,
                    e.summary,
                    e.start_unix,
                    e.end_unix,
                    e.all_day as i64,
                    e.updated_unix,
                    e.etag,
                ],
            )?;
        }
        tx.commit()
    }

    /// Cached events overlapping `[from, to)`, restricted to selected calendars
    /// only — this is the offline-safe read path the Calendar tab renders from.
    pub fn list_events(&self, from: i64, to: i64) -> rusqlite::Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT ce.event_id, ce.calendar_id, ce.summary, ce.start_unix, ce.end_unix, ce.all_day
             FROM cal_events ce
             JOIN calendars c ON c.gcal_id = ce.calendar_id
             WHERE c.selected = 1 AND ce.start_unix < ?2 AND ce.end_unix > ?1
             ORDER BY ce.start_unix",
        )?;
        let rows = stmt.query_map(rusqlite::params![from, to], |r| {
            Ok(EventRow {
                event_id: r.get(0)?,
                calendar_id: r.get(1)?,
                summary: r.get(2)?,
                start_unix: r.get(3)?,
                end_unix: r.get(4)?,
                all_day: r.get::<_, i64>(5)? != 0,
            })
        })?;
        rows.collect()
    }

    /// Read a `meta` kv value (e.g. `google_last_refresh`), or `None` if unset.
    pub fn get_meta(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                rusqlite::params![key],
                |r| r.get(0),
            )
            .optional()
    }

    /// Write a `meta` kv value, upserting.
    pub fn set_meta(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            rusqlite::params![key, value],
        )?;
        Ok(())
    }
}

/// Row shape for [`Store::list_calendars`] — the app-local overlay/selection
/// state layered on top of what Google reports.
pub struct CalendarRow {
    pub gcal_id: String,
    pub summary: String,
    pub bg_color: String,
    pub selected: bool,
    pub is_primary: bool,
}

/// Row shape for [`Store::list_events`] — the offline render cache.
pub struct EventRow {
    pub event_id: String,
    pub calendar_id: String,
    pub summary: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub all_day: bool,
}

/// Persisted `mode_override` label → [`Mode`]; unknown/NULL → `None` (classify at
/// the edge). Matches nudge-svc `persist::parse_mode`.
fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "off_task" => Some(Mode::OffTask),
        "on_task" => Some(Mode::OnTask),
        _ => None,
    }
}

/// Inverse of [`parse_mode`] for the write side.
fn mode_label(m: Option<Mode>) -> Option<&'static str> {
    match m {
        Some(Mode::OffTask) => Some("off_task"),
        Some(Mode::OnTask) => Some("on_task"),
        None => None,
    }
}

/// Signal the resident svc to re-read after a write, via the same named event
/// nudge-ctl uses. Best-effort: if the svc isn't running the event won't exist
/// and we simply return `false` — the next svc boot reads the fresh rows anyway.
#[cfg(windows)]
pub fn signal_reload() -> bool {
    use windows::core::w;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, SetEvent, EVENT_MODIFY_STATE};
    unsafe {
        match OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\nudge-bot-reload")) {
            Ok(h) => {
                let ok = SetEvent(h).is_ok();
                let _ = CloseHandle(h);
                ok
            }
            Err(_) => false,
        }
    }
}

#[cfg(not(windows))]
pub fn signal_reload() -> bool {
    false
}
