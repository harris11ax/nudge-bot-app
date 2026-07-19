//! nudge-app's write side of the app-writer / svc-reader `tasks` contract
//! (UI-PLAN §3). The GUI is the SOLE writer; the resident nudge-svc opens the
//! same `sessions.db` read-only (`persist::Db::tasks`) and re-reads on the
//! `Local\nudge-bot-reload` signal. The CREATE batch here is kept byte-compatible
//! with nudge-svc's `persist.rs` so whichever process touches the file first
//! creates an identical schema.

use crate::google::calendar::{CalendarInfo, EventInfo};
use chrono::{Local, TimeZone, Timelike};
use nudge_core::tasks::{Recur, Task, TriggerSource};
use nudge_core::Mode;
use rusqlite::OptionalExtension;
use std::collections::HashSet;
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

/// Local minutes-since-midnight of a unix instant, in the machine's local
/// timezone — the same clock the svc's `local_now()` samples, so a one-shot
/// task's synthesized window opens at its deadline's wall-clock time. Falls back
/// to 0 for the (impossible on a valid deadline) out-of-range case.
fn local_minutes_of_day(unix: i64) -> u32 {
    match Local.timestamp_opt(unix, 0).single() {
        Some(dt) => dt.hour() * 60 + dt.minute(),
        None => 0,
    }
}

pub struct Store {
    conn: rusqlite::Connection,
}

impl Store {
    /// Open (creating the dir + schema if needed). Idempotent with the svc's own
    /// `Db::open`.
    pub fn open() -> rusqlite::Result<Self> {
        Self::open_at(db_path())
    }

    /// Open (and migrate) the store at an explicit path. Backs [`Self::open`] and
    /// lets tests target a temp DB instead of the machine-wide `db_path()`.
    pub fn open_at(path: PathBuf) -> rusqlite::Result<Self> {
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
                 task_source TEXT    NOT NULL DEFAULT 'manual',
                 gcal_event_id  TEXT,
                 estimate_minutes INTEGER,
                 logged_minutes   INTEGER NOT NULL DEFAULT 0
             );
             -- Shared reader table (§6.4/§6.8): svc reads task-scoped tool lists
             -- at the sample compare, so this stays byte-identical with persist.rs.
             CREATE TABLE IF NOT EXISTS task_tools (
                 task_id  INTEGER NOT NULL,
                 app_name TEXT    NOT NULL,
                 kind     TEXT    NOT NULL DEFAULT 'tool',
                 PRIMARY KEY (task_id, app_name, kind)
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
             );
             -- Global app classification (§6.2): class ∈ favorite|normal|hidden|
             -- not_tool. App-owned; drives the Tools selector and the ON/OFF
             -- classification screen. The svc never reads this.
             CREATE TABLE IF NOT EXISTS app_classes (
                 app_name TEXT PRIMARY KEY,
                 class    TEXT NOT NULL DEFAULT 'normal'
             );
             -- AW usage cache (§6.2 selector sort), refreshed on the GCal cadence.
             -- Bounded to known apps; app-owned.
             CREATE TABLE IF NOT EXISTS app_usage (
                 app_name    TEXT    PRIMARY KEY,
                 minutes_90d INTEGER NOT NULL DEFAULT 0,
                 refreshed_at INTEGER NOT NULL DEFAULT 0
             );
             -- App-only inbox (10d): candidate tasks surfaced from connectors
             -- (Gmail/GCal, 10e) awaiting a user Accept/Dismiss. Distinct from
             -- `tasks` so a connector's guess never lands as a live trigger
             -- without a human in the loop.
             CREATE TABLE IF NOT EXISTS suggested_triggers (
                 id             INTEGER PRIMARY KEY,
                 title          TEXT    NOT NULL,
                 description    TEXT    NOT NULL DEFAULT '',
                 deadline       INTEGER,
                 source         TEXT    NOT NULL DEFAULT 'manual',
                 gcal_event_id  TEXT,
                 status         TEXT    NOT NULL DEFAULT 'pending',
                 created_unix   INTEGER NOT NULL DEFAULT 0
             );",
        )?;
        // §7.1 rename: the user-facing `trigger_source` column is now `task_source`.
        // On a pre-§7.1 DB this renames in place; on a fresh/already-migrated DB the
        // column doesn't exist and the error is ignored (same tolerant idiom below).
        let _ = conn.execute("ALTER TABLE tasks RENAME COLUMN trigger_source TO task_source", []);
        // Additive migration for a pre-Phase-2 `tasks` table (§3), mirroring
        // persist.rs: duplicate-column errors on an already-migrated DB are ignored.
        for col in [
            "ALTER TABLE tasks ADD COLUMN estimate_minutes INTEGER",
            "ALTER TABLE tasks ADD COLUMN logged_minutes INTEGER NOT NULL DEFAULT 0",
        ] {
            let _ = conn.execute(col, []);
        }
        Ok(Self { conn })
    }

    /// All tasks, ordered by id (stable). Tolerant read-back mirrors the svc:
    /// a malformed `recur` → [`Recur::Once`], unknown `task_source` → Manual.
    pub fn list(&self) -> rusqlite::Result<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, description, deadline, task_type, minutes,
                    recur, mode_override, task_source, gcal_event_id,
                    estimate_minutes, logged_minutes
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
                task_source: TriggerSource::from_label(&source_s),
                gcal_event_id: r.get(9)?,
                estimate_minutes: r.get::<_, Option<i64>>(10)?.map(|m| m as u32),
                logged_minutes: r.get::<_, i64>(11)? as u32,
            })
        })?;
        rows.collect()
    }

    /// Insert a task, returning the assigned rowid. `id` on the input is ignored.
    pub fn insert(&self, t: &Task) -> rusqlite::Result<i64> {
        // `logged_minutes` is deliberately omitted: it is svc-owned (§3) and
        // defaults 0 on insert; the app writes only `estimate_minutes`.
        self.conn.execute(
            "INSERT INTO tasks
                (title, description, deadline, task_type, minutes,
                 recur, mode_override, task_source, gcal_event_id, estimate_minutes)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            rusqlite::params![
                t.title,
                t.desc,
                t.deadline,
                t.task_type,
                t.minutes.map(|m| m as i64),
                t.recur.to_spec(),
                mode_label(t.mode_override),
                t.task_source.label(),
                t.gcal_event_id,
                t.estimate_minutes.map(|m| m as i64),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Insert many tasks in a single transaction. Either every row lands or none
    /// do (CSV bulk import, PLAN-csv-import.md §3 P2) — one commit, so the svc is
    /// signalled once by the caller, not per row. Returns the new rowids in order.
    pub fn insert_batch(&mut self, tasks: &[Task]) -> rusqlite::Result<Vec<i64>> {
        let tx = self.conn.transaction()?;
        let mut ids = Vec::with_capacity(tasks.len());
        {
            let mut stmt = tx.prepare(
                "INSERT INTO tasks
                    (title, description, deadline, task_type, minutes,
                     recur, mode_override, task_source, gcal_event_id, estimate_minutes)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for t in tasks {
                stmt.execute(rusqlite::params![
                    t.title,
                    t.desc,
                    t.deadline,
                    t.task_type,
                    t.minutes.map(|m| m as i64),
                    t.recur.to_spec(),
                    mode_label(t.mode_override),
                    t.task_source.label(),
                    t.gcal_event_id,
                    t.estimate_minutes.map(|m| m as i64),
                ])?;
                ids.push(tx.last_insert_rowid());
            }
        }
        tx.commit()?;
        Ok(ids)
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

    /// Upsert a single event locally (optimistic write-before-push, or the
    /// authoritative row echoed back by `events.insert`/`events.update`).
    /// Unlike [`Self::replace_events`] this touches exactly one row.
    pub fn upsert_event(&self, calendar_id: &str, e: &EventInfo) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO cal_events
                (event_id, calendar_id, summary, start_unix, end_unix, all_day, updated_unix, etag)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(event_id) DO UPDATE SET
                 calendar_id  = excluded.calendar_id,
                 summary      = excluded.summary,
                 start_unix   = excluded.start_unix,
                 end_unix     = excluded.end_unix,
                 all_day      = excluded.all_day,
                 updated_unix = excluded.updated_unix,
                 etag         = excluded.etag",
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
        Ok(())
    }

    /// Remove one cached event row (used to drop a create's temporary local id
    /// once the authoritative row from Google is upserted in its place).
    pub fn delete_event(&self, event_id: &str) -> rusqlite::Result<usize> {
        self.conn.execute(
            "DELETE FROM cal_events WHERE event_id = ?1",
            rusqlite::params![event_id],
        )
    }

    /// App-chosen write-target calendar (`meta.primary_gcal_id`, 10c's picker).
    /// Falls back to whichever calendar Google itself reports as primary
    /// (`calendars.is_primary`) if the user hasn't picked one explicitly yet.
    pub fn primary_calendar_id(&self) -> rusqlite::Result<Option<String>> {
        if let Some(id) = self.get_meta("primary_gcal_id")? {
            return Ok(Some(id));
        }
        self.conn
            .query_row(
                "SELECT gcal_id FROM calendars WHERE is_primary = 1 LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()
    }

    /// Set the app-chosen write-target calendar (Settings — Calendar picker).
    pub fn set_primary_calendar(&self, gcal_id: &str) -> rusqlite::Result<()> {
        self.set_meta("primary_gcal_id", gcal_id)
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

    /// Pending suggested triggers (Triggers tab — Suggested section), newest first.
    pub fn list_suggested_triggers(&self) -> rusqlite::Result<Vec<SuggestedTriggerRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, title, description, deadline, source, gcal_event_id, created_unix
             FROM suggested_triggers WHERE status = 'pending' ORDER BY id DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(SuggestedTriggerRow {
                id: r.get(0)?,
                title: r.get(1)?,
                description: r.get(2)?,
                deadline: r.get(3)?,
                source: r.get(4)?,
                gcal_event_id: r.get(5)?,
                created_unix: r.get(6)?,
            })
        })?;
        rows.collect()
    }

    /// Insert a fresh suggestion (connector ingest, 10e). Returns the assigned rowid.
    pub fn insert_suggested_trigger(
        &self,
        title: &str,
        description: &str,
        deadline: Option<i64>,
        source: &str,
        gcal_event_id: Option<&str>,
        created_unix: i64,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO suggested_triggers
                (title, description, deadline, source, gcal_event_id, status, created_unix)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6)",
            rusqlite::params![title, description, deadline, source, gcal_event_id, created_unix],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Accept a suggestion: insert it into `tasks` (task_source carried over),
    /// mark the suggestion accepted, and return the new task's rowid. Rejects if
    /// the suggestion isn't pending (already accepted/dismissed, or unknown id).
    pub fn accept_suggested_trigger(&self, id: i64) -> rusqlite::Result<i64> {
        let row: SuggestedTriggerRow = self.conn.query_row(
            "SELECT id, title, description, deadline, source, gcal_event_id, created_unix
             FROM suggested_triggers WHERE id = ?1 AND status = 'pending'",
            rusqlite::params![id],
            |r| {
                Ok(SuggestedTriggerRow {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    description: r.get(2)?,
                    deadline: r.get(3)?,
                    source: r.get(4)?,
                    gcal_event_id: r.get(5)?,
                    created_unix: r.get(6)?,
                })
            },
        )?;
        // A one-shot task schedules by local minutes-since-midnight on its
        // deadline's date (see nudge-core `schedule::Win::from_task`). Derive that
        // time-of-day from the deadline so the accepted suggestion actually fires;
        // a deadline-less suggestion (e.g. undated Gmail) stays a planner-only row.
        let minutes = row.deadline.map(local_minutes_of_day);
        let task = Task {
            id: None,
            title: row.title,
            desc: row.description,
            deadline: row.deadline,
            task_type: String::new(),
            minutes,
            recur: Recur::Once,
            mode_override: None,
            task_source: TriggerSource::from_label(&row.source),
            gcal_event_id: row.gcal_event_id,
            estimate_minutes: None,
            logged_minutes: 0,
        };
        let task_id = self.insert(&task)?;
        self.conn.execute(
            "UPDATE suggested_triggers SET status = 'accepted' WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(task_id)
    }

    /// Every Google Calendar event id already claimed — either mirrored as a
    /// task or sitting as a pending suggestion. The connector (10e) checks this
    /// before depositing a gcal candidate so an event never doubles up across a
    /// live task and the inbox, or across successive connector runs.
    pub fn known_gcal_event_ids(&self) -> rusqlite::Result<HashSet<String>> {
        let mut set = HashSet::new();
        let mut stmt = self.conn.prepare(
            "SELECT gcal_event_id FROM tasks WHERE gcal_event_id IS NOT NULL
             UNION
             SELECT gcal_event_id FROM suggested_triggers
                 WHERE gcal_event_id IS NOT NULL AND status = 'pending'",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for id in rows {
            set.insert(id?);
        }
        Ok(set)
    }

    /// Titles of pending suggestions from a given `source` (e.g. `"gmail"`).
    /// Gmail candidates carry no stable external id in this schema, so the
    /// connector dedups them by title — re-scanning the same email yields the
    /// same title and is skipped.
    pub fn pending_titles_for_source(&self, source: &str) -> rusqlite::Result<HashSet<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT title FROM suggested_triggers WHERE source = ?1 AND status = 'pending'",
        )?;
        let rows = stmt.query_map(rusqlite::params![source], |r| r.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<HashSet<String>>>()
    }

    /// Dismiss a suggestion without creating a task.
    pub fn dismiss_suggested_trigger(&self, id: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE suggested_triggers SET status = 'dismissed' WHERE id = ?1",
            rusqlite::params![id],
        )?;
        Ok(())
    }

    // --- Task tools + app classification (§6.2/§6.4, Phase 2 schema) ---

    /// Replace a task's tool list wholesale (§6.4). Clears the task's rows then
    /// re-inserts `(app_name, kind)` pairs, deduping via the composite PK. A
    /// transaction so a partial write can't leave a half-updated list the svc
    /// might read mid-edit.
    pub fn set_task_tools(&mut self, task_id: i64, tools: &[(String, String)]) -> rusqlite::Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM task_tools WHERE task_id = ?1", rusqlite::params![task_id])?;
        for (app_name, kind) in tools {
            tx.execute(
                "INSERT OR IGNORE INTO task_tools (task_id, app_name, kind) VALUES (?1, ?2, ?3)",
                rusqlite::params![task_id, app_name, kind],
            )?;
        }
        tx.commit()
    }

    /// A task's `(app_name, kind)` tool rows, ordered for a stable UI/compare.
    pub fn list_task_tools(&self, task_id: i64) -> rusqlite::Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT app_name, kind FROM task_tools WHERE task_id = ?1 ORDER BY app_name, kind",
        )?;
        let rows = stmt.query_map(rusqlite::params![task_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect()
    }

    /// Set an app's global class (§6.2 favorite|normal|hidden|not_tool), upserting.
    pub fn set_app_class(&self, app_name: &str, class: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO app_classes (app_name, class) VALUES (?1, ?2)
             ON CONFLICT(app_name) DO UPDATE SET class = excluded.class",
            rusqlite::params![app_name, class],
        )?;
        Ok(())
    }

    /// An app's global class, or `None` if unclassified (treated as `normal`).
    pub fn app_class(&self, app_name: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT class FROM app_classes WHERE app_name = ?1",
                rusqlite::params![app_name],
                |r| r.get(0),
            )
            .optional()
    }

    /// Upsert an app's 90-day usage minutes + refresh time (§6.2 selector sort).
    pub fn upsert_app_usage(&self, app_name: &str, minutes_90d: i64, refreshed_at: i64) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO app_usage (app_name, minutes_90d, refreshed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(app_name) DO UPDATE SET
                 minutes_90d = excluded.minutes_90d,
                 refreshed_at = excluded.refreshed_at",
            rusqlite::params![app_name, minutes_90d, refreshed_at],
        )?;
        Ok(())
    }

    /// Every app the selector can offer (§6.2): the union of usage-cached apps
    /// and explicitly-classified apps, usage-sorted descending. An app in only
    /// one table still appears (`class` defaults `'normal'`, `minutes_90d` 0).
    /// The frontend does the favorite-pinning / hidden-filtering — this is the
    /// raw, complete list.
    pub fn list_apps_for_selector(&self) -> rusqlite::Result<Vec<AppRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT app_name, MAX(minutes_90d) AS minutes_90d, MAX(class) AS class FROM (
                 SELECT u.app_name, u.minutes_90d, COALESCE(c.class, 'normal') AS class
                   FROM app_usage u LEFT JOIN app_classes c ON c.app_name = u.app_name
                 UNION ALL
                 SELECT c.app_name, COALESCE(u.minutes_90d, 0), c.class
                   FROM app_classes c LEFT JOIN app_usage u ON u.app_name = c.app_name
             ) GROUP BY app_name
             ORDER BY minutes_90d DESC, app_name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AppRow { name: r.get(0)?, minutes_90d: r.get(1)?, class: r.get(2)? })
        })?;
        rows.collect()
    }

    /// Explicitly-classified apps only (Settings — Tools tab), usage-sorted.
    pub fn list_app_classes(&self) -> rusqlite::Result<Vec<AppRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT c.app_name, COALESCE(u.minutes_90d, 0), c.class
             FROM app_classes c LEFT JOIN app_usage u ON u.app_name = c.app_name
             ORDER BY COALESCE(u.minutes_90d, 0) DESC, c.app_name",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AppRow { name: r.get(0)?, minutes_90d: r.get(1)?, class: r.get(2)? })
        })?;
        rows.collect()
    }

    /// Not-Tool recommendation seed (§6.2 Settings): high-usage apps that have
    /// never appeared in any task's tool/ignore list and aren't already
    /// classified. Bounded so the Settings list stays scannable.
    pub fn list_not_tool_candidates(&self, limit: i64) -> rusqlite::Result<Vec<AppRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT u.app_name, u.minutes_90d, 'normal'
             FROM app_usage u
             WHERE u.app_name NOT IN (SELECT app_name FROM task_tools)
               AND u.app_name NOT IN (SELECT app_name FROM app_classes)
             ORDER BY u.minutes_90d DESC, u.app_name
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![limit], |r| {
            Ok(AppRow { name: r.get(0)?, minutes_90d: r.get(1)?, class: r.get(2)? })
        })?;
        rows.collect()
    }

    /// Per-task ignore rows across ALL tasks (Settings read-only view):
    /// `(task_id, task_title, app_name)` ordered by task then app.
    pub fn list_all_ignores(&self) -> rusqlite::Result<Vec<(i64, String, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT tt.task_id, COALESCE(t.title, '(deleted task)'), tt.app_name
             FROM task_tools tt LEFT JOIN tasks t ON t.id = tt.task_id
             WHERE tt.kind = 'ignore'
             ORDER BY tt.task_id, tt.app_name",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }
}

/// Row shape for the §6.2 selector/Settings app lists.
pub struct AppRow {
    pub name: String,
    pub minutes_90d: i64,
    pub class: String,
}

/// Row shape for [`Store::list_suggested_triggers`] — a connector-surfaced
/// candidate task awaiting user Accept/Dismiss.
pub struct SuggestedTriggerRow {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub deadline: Option<i64>,
    pub source: String,
    pub gcal_event_id: Option<String>,
    pub created_unix: i64,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> (Store, PathBuf) {
        let mut p = std::env::temp_dir();
        p.push(format!("nudge-app-db-test-{}-{tag}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        (Store::open_at(p.clone()).unwrap(), p)
    }

    // A task round-trips its estimate; logged_minutes defaults 0 on the app write
    // side (svc-owned). Tools round-trip through the child table.
    #[test]
    fn task_estimate_and_tools_round_trip() {
        let (mut store, path) = temp_store("tools");
        let t = Task {
            id: None,
            title: "write".into(),
            desc: String::new(),
            deadline: Some(999),
            task_type: String::new(),
            minutes: None,
            recur: Recur::Once,
            mode_override: None,
            task_source: TriggerSource::Manual,
            gcal_event_id: None,
            estimate_minutes: Some(90),
            logged_minutes: 7, // ignored by insert (svc-owned)
        };
        let id = store.insert(&t).unwrap();
        let got = &store.list().unwrap()[0];
        assert_eq!(got.estimate_minutes, Some(90));
        assert_eq!(got.logged_minutes, 0); // insert never writes logged

        store
            .set_task_tools(id, &[("code.exe".into(), "tool".into()), ("slack.exe".into(), "ignore".into())])
            .unwrap();
        assert_eq!(
            store.list_task_tools(id).unwrap(),
            vec![("code.exe".into(), "tool".into()), ("slack.exe".into(), "ignore".into())]
        );
        // Replace is wholesale.
        store.set_task_tools(id, &[("code.exe".into(), "tool".into())]).unwrap();
        assert_eq!(store.list_task_tools(id).unwrap().len(), 1);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn app_class_and_usage_upsert() {
        let (store, path) = temp_store("class");
        assert_eq!(store.app_class("code.exe").unwrap(), None);
        store.set_app_class("code.exe", "favorite").unwrap();
        assert_eq!(store.app_class("code.exe").unwrap().as_deref(), Some("favorite"));
        store.set_app_class("code.exe", "hidden").unwrap(); // upsert
        assert_eq!(store.app_class("code.exe").unwrap().as_deref(), Some("hidden"));

        store.upsert_app_usage("code.exe", 120, 1000).unwrap();
        store.upsert_app_usage("code.exe", 200, 2000).unwrap();
        let (m, r): (i64, i64) = store
            .conn
            .query_row(
                "SELECT minutes_90d, refreshed_at FROM app_usage WHERE app_name = 'code.exe'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!((m, r), (200, 2000));

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    // Selector list is the union of usage + classes, usage-sorted; not-tool
    // candidates exclude anything already a task tool or already classified.
    #[test]
    fn selector_and_not_tool_candidate_queries() {
        let (mut store, path) = temp_store("selector");
        store.upsert_app_usage("code.exe", 500, 1).unwrap();
        store.upsert_app_usage("chrome.exe", 300, 1).unwrap();
        store.upsert_app_usage("game.exe", 200, 1).unwrap();
        store.set_app_class("code.exe", "favorite").unwrap();
        store.set_app_class("obscure.exe", "hidden").unwrap(); // classified, no usage

        let apps = store.list_apps_for_selector().unwrap();
        let names: Vec<&str> = apps.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["code.exe", "chrome.exe", "game.exe", "obscure.exe"]);
        assert_eq!(apps[0].class, "favorite");
        assert_eq!(apps[1].class, "normal"); // usage-only app defaults normal
        assert_eq!(apps[3].minutes_90d, 0); // class-only app defaults 0 usage

        let classed = store.list_app_classes().unwrap();
        assert_eq!(classed.len(), 2);

        // chrome + game are unclassified non-tools; making chrome a task tool
        // removes it from the candidate list.
        let t = Task {
            id: None,
            title: "t".into(),
            desc: String::new(),
            deadline: None,
            task_type: String::new(),
            minutes: None,
            recur: Recur::Once,
            mode_override: None,
            task_source: TriggerSource::Manual,
            gcal_event_id: None,
            estimate_minutes: None,
            logged_minutes: 0,
        };
        let id = store.insert(&t).unwrap();
        store.set_task_tools(id, &[("chrome.exe".into(), "tool".into()), ("slack.exe".into(), "ignore".into())]).unwrap();
        let cands = store.list_not_tool_candidates(10).unwrap();
        let cand_names: Vec<&str> = cands.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(cand_names, vec!["game.exe"]);

        // Ignore rows surface with their task title.
        let ignores = store.list_all_ignores().unwrap();
        assert_eq!(ignores, vec![(id, "t".to_string(), "slack.exe".to_string())]);

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    // CSV bulk import (P2): a batch lands atomically in one transaction, returns
    // rowids in input order, and every field round-trips through `list()`.
    #[test]
    fn insert_batch_lands_all_rows_in_order() {
        let (mut store, path) = temp_store("batch");
        let mk = |title: &str, deadline: Option<i64>, mins: Option<u32>| Task {
            id: None,
            title: title.into(),
            desc: format!("{title} desc"),
            deadline,
            task_type: "work".into(),
            minutes: mins,
            recur: Recur::Once,
            mode_override: Some(Mode::OffTask),
            task_source: TriggerSource::Manual,
            gcal_event_id: None,
            estimate_minutes: Some(45),
            logged_minutes: 0,
        };
        let batch = vec![
            mk("alpha", Some(1000), Some(600)),
            mk("beta", None, None),
            mk("gamma", Some(2000), Some(90)),
        ];
        let ids = store.insert_batch(&batch).unwrap();
        assert_eq!(ids.len(), 3);
        // Rowids are assigned in input order and strictly increasing.
        assert!(ids[0] < ids[1] && ids[1] < ids[2], "ids: {ids:?}");

        let all = store.list().unwrap();
        assert_eq!(all.len(), 3);
        let titles: Vec<&str> = all.iter().map(|t| t.title.as_str()).collect();
        assert!(titles.contains(&"alpha") && titles.contains(&"beta") && titles.contains(&"gamma"));
        let beta = all.iter().find(|t| t.title == "beta").unwrap();
        assert_eq!(beta.deadline, None);
        assert_eq!(beta.minutes, None);
        assert_eq!(beta.estimate_minutes, Some(45));
        assert_eq!(beta.mode_override, Some(Mode::OffTask));
        assert_eq!(beta.logged_minutes, 0); // svc-owned, never written by batch

        drop(store);
        let _ = std::fs::remove_file(&path);
    }

    // An empty batch is a no-op that commits cleanly and touches no rows.
    #[test]
    fn insert_batch_empty_is_noop() {
        let (mut store, path) = temp_store("batch-empty");
        let ids = store.insert_batch(&[]).unwrap();
        assert!(ids.is_empty());
        assert_eq!(store.list().unwrap().len(), 0);
        drop(store);
        let _ = std::fs::remove_file(&path);
    }
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
