//! SQLite session log. Append-only writes at state edges only; WAL off,
//! small page cache. Tables: sessions (state edges) and outcomes (user
//! answers: started/snoozed/skipped/checked_in).

use nudge_core::tasks::{Recur, Task, TriggerSource};
use nudge_core::{Mode, UnixTime};
use std::path::PathBuf;

pub struct Db {
    conn: rusqlite::Connection,
}

impl Db {
    pub fn open(path: PathBuf) -> Self {
        std::fs::create_dir_all(path.parent().unwrap()).expect("config dir");
        let conn = rusqlite::Connection::open(&path).expect("open sessions.db");
        conn.execute_batch(
            "PRAGMA journal_mode=DELETE; PRAGMA cache_size=-64;
             CREATE TABLE IF NOT EXISTS sessions (
                 at    INTEGER NOT NULL,
                 state TEXT    NOT NULL
             );
             CREATE TABLE IF NOT EXISTS outcomes (
                 at      INTEGER NOT NULL,
                 outcome TEXT    NOT NULL
             );
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
                 gcal_event_id  TEXT,
                 estimate_minutes INTEGER,
                 logged_minutes   INTEGER NOT NULL DEFAULT 0
             );
             -- Task-scoped tool lists (§6.4/§6.8). `kind`: 'tool' (an app that
             -- counts as working on this task) or 'ignore' (task-scoped transient
             -- mute). App is the writer; the svc reads it at the sample compare
             -- (Phase 3), so it lives in both schemas byte-identical like `tasks`.
             CREATE TABLE IF NOT EXISTS task_tools (
                 task_id  INTEGER NOT NULL,
                 app_name TEXT    NOT NULL,
                 kind     TEXT    NOT NULL DEFAULT 'tool',
                 PRIMARY KEY (task_id, app_name, kind)
             );
             -- Global app classification (§6.2). `class`: favorite / normal /
             -- hidden / not_tool. App-owned; drives the Tools selector's ordering
             -- and hides never-relevant apps from the sample compare.
             CREATE TABLE IF NOT EXISTS app_classes (
                 app_name TEXT PRIMARY KEY,
                 class    TEXT NOT NULL DEFAULT 'normal'
             );
             -- AW usage cache (§6.2 selector sort). Refreshed on the GCal cadence
             -- (Phase 4+); bounded to known apps, so it stays tiny.
             CREATE TABLE IF NOT EXISTS app_usage (
                 app_name     TEXT PRIMARY KEY,
                 minutes_90d  INTEGER NOT NULL DEFAULT 0,
                 refreshed_at INTEGER
             );",
        )
        .expect("schema");
        // Additive migration for a pre-Phase-2 `tasks` table (§3): SQLite has no
        // ADD COLUMN IF NOT EXISTS, so a duplicate-column error on an already-
        // migrated DB is expected and ignored — same idiom as the `sessions`
        // widening above. New rows read the documented defaults (estimate NULL →
        // "no estimate", logged 0).
        for col in [
            "ALTER TABLE tasks ADD COLUMN estimate_minutes INTEGER",
            "ALTER TABLE tasks ADD COLUMN logged_minutes INTEGER NOT NULL DEFAULT 0",
        ] {
            let _ = conn.execute(col, []);
        }
        // UI-P0 (8c): widen `sessions` with the notification mode chosen at a
        // prompt edge, the edge kind the svc armed next, and a click-through flag
        // reserved for P1's notification-click wiring. Added incrementally so an
        // existing sessions.db upgrades in place; SQLite has no ADD COLUMN IF NOT
        // EXISTS, so a duplicate-column error on a second boot is expected and
        // ignored.
        for col in [
            "ALTER TABLE sessions ADD COLUMN mode         TEXT",
            "ALTER TABLE sessions ADD COLUMN edge_kind    TEXT",
            "ALTER TABLE sessions ADD COLUMN clickthrough INTEGER",
        ] {
            let _ = conn.execute(col, []);
        }
        // Record the schema generation (§3 additive migration guard). The
        // ADD-COLUMN idiom above already makes every migration idempotent; this
        // stamp is the forward-compat marker future destructive migrations would
        // branch on. 2 = Phase-2 shape (estimate/logged + task_tools/app_classes/
        // app_usage).
        let _ = conn.execute_batch("PRAGMA user_version = 2;");
        Self { conn }
    }

    /// Append a state-transition row. `mode` is the notification mode the prompt
    /// opened in (`None` on non-prompt edges); `armed_kind` is the [`EdgeKind`]
    /// label of the timer armed alongside this edge (`None` if none was armed).
    pub fn log_edge(
        &mut self,
        entered: &str,
        at: UnixTime,
        mode: Option<&str>,
        armed_kind: Option<&str>,
    ) {
        self.conn
            .execute(
                "INSERT INTO sessions (at, state, mode, edge_kind) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![at, entered, mode, armed_kind],
            )
            .expect("log edge");
    }

    /// Append a user-outcome row (started/snoozed/skipped/checked_in). `outcome`
    /// is the stable [`nudge_core::state::Outcome::label`] string.
    pub fn log_outcome(&mut self, outcome: &str, at: UnixTime) {
        self.conn
            .execute(
                "INSERT INTO outcomes (at, outcome) VALUES (?1, ?2)",
                rusqlite::params![at, outcome],
            )
            .expect("log outcome");
    }

    /// Mark a notification click-through on the most recent session row (P1).
    /// The `clickthrough` column was reserved at 8c; the overlay body-click sets
    /// it here. Targets the latest `sessions` row — the prompt the user just
    /// clicked — by rowid. Best-effort: no prompt row yet is a silent no-op.
    pub fn log_clickthrough(&mut self, _at: UnixTime) {
        let _ = self.conn.execute(
            "UPDATE sessions SET clickthrough = 1
             WHERE rowid = (SELECT MAX(rowid) FROM sessions)",
            [],
        );
    }

    /// Read every row of the `tasks` table (UI-P1). The svc is a **read-only**
    /// consumer here: nudge-app is the sole writer and signals changes via the
    /// existing `Local\nudge-bot-reload` event, at which point the svc re-reads.
    /// A malformed `recur` spec falls back to [`Recur::Once`] rather than dropping
    /// the row, mirroring the tolerant `trigger_source` read-back. Ordered by id
    /// so callers see a stable sequence. The svc loop feeds these rows to
    /// [`nudge_core::schedule::context_with_tasks`] each wake (9e window-gen).
    pub fn tasks(&self) -> Vec<Task> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, title, description, deadline, task_type, minutes,
                        recur, mode_override, trigger_source, gcal_event_id,
                        estimate_minutes, logged_minutes
                 FROM tasks ORDER BY id",
            )
            .expect("prepare tasks");
        let rows = stmt
            .query_map([], |r| {
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
                    estimate_minutes: r.get::<_, Option<i64>>(10)?.map(|m| m as u32),
                    logged_minutes: r.get::<_, i64>(11)? as u32,
                })
            })
            .expect("query tasks");
        rows.map(|r| r.expect("task row")).collect()
    }

    /// Read a task's tool list (§6.4/§6.8), as `(app_name, kind)` pairs where
    /// `kind` is `"tool"` or `"ignore"`. The svc reads this read-only at the
    /// Phase-3 sample compare (foreground app ∈ this task's tools?); the app is
    /// the sole writer. Ordered by `app_name` for a stable sequence.
    pub fn task_tools(&self, task_id: i64) -> Vec<(String, String)> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT app_name, kind FROM task_tools
                 WHERE task_id = ?1 ORDER BY app_name",
            )
            .expect("prepare task_tools");
        let rows = stmt
            .query_map([task_id], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query task_tools");
        rows.map(|r| r.expect("task_tool row")).collect()
    }

    /// Global classification for an app (§6.2): `favorite` / `normal` / `hidden`
    /// / `not_tool`. Unset → `None` (treated as `normal`). App-owned; read-only
    /// here.
    pub fn app_class(&self, app_name: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT class FROM app_classes WHERE app_name = ?1",
                [app_name],
                |r| r.get(0),
            )
            .ok()
    }

    /// The **one** sanctioned svc write into `tasks` (§3, budget callout): update
    /// a single row's `logged_minutes` cache by rowid at a sample/check-in/ack
    /// edge. Keyed by rowid, one UPDATE, no schema churn. Every other `tasks`
    /// column is app-owned and read-only here. Returns rows affected (0 = no such
    /// task).
    pub fn set_logged_minutes(&mut self, task_id: i64, minutes: u32) -> usize {
        self.conn
            .execute(
                "UPDATE tasks SET logged_minutes = ?2 WHERE id = ?1",
                rusqlite::params![task_id, minutes as i64],
            )
            .expect("set logged_minutes")
    }
}

/// Map a persisted `mode_override` label to a [`Mode`]; unknown/NULL → `None`
/// (classify at the edge as usual).
fn parse_mode(s: &str) -> Option<Mode> {
    match s {
        "off_task" => Some(Mode::OffTask),
        "on_task" => Some(Mode::OnTask),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unique per-process temp db so parallel test runs don't collide; cleaned up
    // at the end of each test.
    fn temp_db(tag: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("nudge-tasks-test-{}-{tag}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn tasks_migrate_and_round_trip() {
        let path = temp_db("roundtrip");
        let db = Db::open(path.clone());
        // App is the writer in production; the test stands in for it via raw SQL
        // against the same schema the migration created.
        db.conn
            .execute(
                "INSERT INTO tasks
                    (title, description, deadline, task_type, minutes,
                     recur, mode_override, trigger_source, gcal_event_id)
                 VALUES ('gym', 'leg day', 1234, 'health', 1050,
                         'mon,wed,fri', 'on_task', 'gcal', 'evt_9')",
                [],
            )
            .unwrap();
        // A second row exercising the NULL / default paths.
        db.conn
            .execute(
                "INSERT INTO tasks (title, recur) VALUES ('call mum', 'once')",
                [],
            )
            .unwrap();

        let tasks = db.tasks();
        assert_eq!(tasks.len(), 2);

        let gym = &tasks[0];
        assert_eq!(gym.title, "gym");
        assert_eq!(gym.desc, "leg day");
        assert_eq!(gym.deadline, Some(1234));
        assert_eq!(gym.task_type, "health");
        assert_eq!(gym.minutes, Some(1050));
        assert_eq!(gym.recur, Recur::Weekly(vec![0, 2, 4]));
        assert_eq!(gym.mode_override, Some(Mode::OnTask));
        assert_eq!(gym.trigger_source, TriggerSource::Gcal);
        assert_eq!(gym.gcal_event_id.as_deref(), Some("evt_9"));
        assert_eq!(gym.estimate_minutes, None); // unset → no estimate
        assert_eq!(gym.logged_minutes, 0); // column DEFAULT 0

        let mum = &tasks[1];
        assert_eq!(mum.desc, ""); // column DEFAULT ''
        assert_eq!(mum.deadline, None);
        assert_eq!(mum.minutes, None);
        assert_eq!(mum.mode_override, None); // NULL → classify at edge
        assert_eq!(mum.trigger_source, TriggerSource::Manual); // column DEFAULT
        assert_eq!(mum.gcal_event_id, None);
        assert_eq!(mum.logged_minutes, 0);

        // Re-opening must be idempotent (CREATE TABLE IF NOT EXISTS) and preserve rows.
        drop(db);
        let db2 = Db::open(path.clone());
        assert_eq!(db2.tasks().len(), 2);

        drop(db2);
        let _ = std::fs::remove_file(&path);
    }

    // The one sanctioned svc→tasks write (§3): a single UPDATE-by-rowid of
    // `logged_minutes`, leaving every other column untouched.
    #[test]
    fn set_logged_minutes_by_rowid() {
        let path = temp_db("setlogged");
        let mut db = Db::open(path.clone());
        db.conn
            .execute(
                "INSERT INTO tasks (title, estimate_minutes) VALUES ('write', 120)",
                [],
            )
            .unwrap();
        let id = db.conn.last_insert_rowid();

        assert_eq!(db.set_logged_minutes(id, 45), 1);
        let t = &db.tasks()[0];
        assert_eq!(t.logged_minutes, 45);
        assert_eq!(t.estimate_minutes, Some(120)); // untouched
        assert_eq!(t.title, "write"); // untouched

        // Unknown id affects nothing.
        assert_eq!(db.set_logged_minutes(9999, 10), 0);

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    // task_tools round-trips per task, carrying tool vs. ignore kinds, and the
    // reader scopes to the requested task_id.
    #[test]
    fn task_tools_round_trip() {
        let path = temp_db("tooltbl");
        let db = Db::open(path.clone());
        db.conn
            .execute_batch(
                "INSERT INTO task_tools (task_id, app_name, kind) VALUES
                    (1, 'code.exe', 'tool'),
                    (1, 'chrome.exe', 'tool'),
                    (1, 'slack.exe', 'ignore'),
                    (2, 'blender.exe', 'tool');",
            )
            .unwrap();

        let t1 = db.task_tools(1);
        assert_eq!(
            t1,
            vec![
                ("chrome.exe".into(), "tool".into()),
                ("code.exe".into(), "tool".into()),
                ("slack.exe".into(), "ignore".into()),
            ]
        );
        assert_eq!(db.task_tools(2), vec![("blender.exe".into(), "tool".into())]);
        assert!(db.task_tools(3).is_empty());

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    // app_classes / app_usage tables exist and round-trip; unset class → None.
    #[test]
    fn app_classes_and_usage_round_trip() {
        let path = temp_db("appmeta");
        let db = Db::open(path.clone());
        db.conn
            .execute_batch(
                "INSERT INTO app_classes (app_name, class) VALUES
                    ('code.exe', 'favorite'), ('game.exe', 'not_tool');
                 INSERT INTO app_usage (app_name, minutes_90d, refreshed_at)
                    VALUES ('code.exe', 4200, 1234);",
            )
            .unwrap();

        assert_eq!(db.app_class("code.exe").as_deref(), Some("favorite"));
        assert_eq!(db.app_class("game.exe").as_deref(), Some("not_tool"));
        assert_eq!(db.app_class("unknown.exe"), None);

        // app_usage is app-side (selector sort); assert the row landed.
        let mins: i64 = db
            .conn
            .query_row(
                "SELECT minutes_90d FROM app_usage WHERE app_name = 'code.exe'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mins, 4200);

        // Schema generation stamped.
        let ver: i64 = db
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(ver, 2);

        drop(db);
        let _ = std::fs::remove_file(&path);
    }

    // A pre-Phase-2 `tasks` table (no estimate/logged columns) migrates in place
    // on open: the ALTER adds the columns and old rows read documented defaults.
    #[test]
    fn migrates_pre_phase2_tasks_table() {
        let path = temp_db("migrate");
        // Stand up the OLD schema (Phase-1 shape) directly and seed a row.
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE tasks (
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
                 INSERT INTO tasks (title) VALUES ('legacy');",
            )
            .unwrap();
        }
        // Opening runs the additive migration; the legacy row survives with defaults.
        let db = Db::open(path.clone());
        let tasks = db.tasks();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].title, "legacy");
        assert_eq!(tasks[0].estimate_minutes, None);
        assert_eq!(tasks[0].logged_minutes, 0);

        drop(db);
        let _ = std::fs::remove_file(&path);
    }
}
