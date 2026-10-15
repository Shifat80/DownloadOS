//! SQLite-backed task store: schema/migrations, persistence, and recovery.
//!
//! The store is the single source of truth for task lifecycle (see
//! `docs/SYSTEM_DESIGN.md` §5.4, §9). All state changes go through the
//! validated transition in [`state::TaskState`]. Access is serialized behind a
//! mutex; rusqlite is synchronous and the store is cheap to call.

pub mod state;

use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub use state::TaskState;

/// Current schema version. Bump when adding a migration step.
const SCHEMA_VERSION: i64 = 1;

/// How bytes are transferred. Segmented mode arrives in Phase 3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadMode {
    Single,
    Segmented,
}

impl DownloadMode {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadMode::Single => "single",
            DownloadMode::Segmented => "segmented",
        }
    }
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "single" => Some(DownloadMode::Single),
            "segmented" => Some(DownloadMode::Segmented),
            _ => None,
        }
    }
}

/// A persisted download task. Byte counts and timestamps are stored as SQLite
/// integers (i64); timestamps are Unix seconds.
///
/// `source_url` may contain signed query parameters and is treated as a
/// secret: it is stored locally to enable resume but must never be logged.
/// `display_url` is the redacted form safe for logs and the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: String,
    pub source_url: String,
    pub display_url: String,
    pub filename: String,
    pub destination_path: String,
    pub state: TaskState,
    pub total_bytes: Option<i64>,
    pub downloaded_bytes: i64,
    pub mode: DownloadMode,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub retry_count: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_error_code: Option<String>,
}

impl DownloadTask {
    /// Build a fresh task in the `Queued` state with timestamps set to now.
    pub fn new_queued(
        id: impl Into<String>,
        source_url: impl Into<String>,
        display_url: impl Into<String>,
        filename: impl Into<String>,
        destination_path: impl Into<String>,
        mode: DownloadMode,
    ) -> Self {
        let now = now_unix();
        Self {
            id: id.into(),
            source_url: source_url.into(),
            display_url: display_url.into(),
            filename: filename.into(),
            destination_path: destination_path.into(),
            state: TaskState::Queued,
            total_bytes: None,
            downloaded_bytes: 0,
            mode,
            etag: None,
            last_modified: None,
            retry_count: 0,
            created_at: now,
            updated_at: now,
            last_error_code: None,
        }
    }
}

/// Errors returned by the task store.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("task not found: {0}")]
    NotFound(String),
    #[error("invalid state transition from {from} to {to}")]
    InvalidTransition {
        from: &'static str,
        to: &'static str,
    },
}

/// The task store. Clone-free; share via `Arc` / Tauri managed state.
pub struct TaskStore {
    conn: Mutex<Connection>,
}

impl TaskStore {
    /// Open (creating if needed) a store at `path` and run migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::from_conn(conn)
    }

    /// Open an in-memory store (used by tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::from_conn(conn)
    }

    fn from_conn(conn: Connection) -> Result<Self, StoreError> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Insert a new task.
    pub fn create(&self, task: &DownloadTask) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        conn.execute(
            "INSERT INTO tasks (
                id, source_url, display_url, filename, destination_path, state,
                total_bytes, downloaded_bytes, mode, etag, last_modified,
                retry_count, created_at, updated_at, last_error_code
             ) VALUES (
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15
             )",
            params![
                task.id,
                task.source_url,
                task.display_url,
                task.filename,
                task.destination_path,
                task.state.as_str(),
                task.total_bytes,
                task.downloaded_bytes,
                task.mode.as_str(),
                task.etag,
                task.last_modified,
                task.retry_count,
                task.created_at,
                task.updated_at,
                task.last_error_code,
            ],
        )?;
        Ok(())
    }

    /// Fetch one task by id.
    pub fn get(&self, id: &str) -> Result<Option<DownloadTask>, StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let task = conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"),
                params![id],
                row_to_task,
            )
            .optional()?;
        Ok(task)
    }

    /// List all tasks, newest first.
    pub fn list(&self) -> Result<Vec<DownloadTask>, StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let mut stmt = conn.prepare(&format!(
            "SELECT {COLUMNS} FROM tasks ORDER BY created_at DESC, id"
        ))?;
        let rows = stmt.query_map([], row_to_task)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    /// Move a task to a new state, validating the transition.
    pub fn update_state(&self, id: &str, to: TaskState) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let current: Option<String> = conn
            .query_row("SELECT state FROM tasks WHERE id = ?1", params![id], |r| {
                r.get(0)
            })
            .optional()?;
        let current = current.ok_or_else(|| StoreError::NotFound(id.to_string()))?;
        let from = TaskState::from_db(&current)
            .ok_or_else(|| StoreError::NotFound(format!("{id} (corrupt state)")))?;

        if !from.can_transition(to) {
            return Err(StoreError::InvalidTransition {
                from: from.as_str(),
                to: to.as_str(),
            });
        }
        conn.execute(
            "UPDATE tasks SET state = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, to.as_str(), now_unix()],
        )?;
        Ok(())
    }

    /// Update byte progress for a task.
    pub fn update_progress(
        &self,
        id: &str,
        downloaded_bytes: i64,
        total_bytes: Option<i64>,
    ) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let n = conn.execute(
            "UPDATE tasks SET downloaded_bytes = ?2, total_bytes = ?3, updated_at = ?4
             WHERE id = ?1",
            params![id, downloaded_bytes, total_bytes, now_unix()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Record the last structured error code for a task.
    pub fn set_error(&self, id: &str, code: &str) -> Result<(), StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let n = conn.execute(
            "UPDATE tasks SET last_error_code = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, code, now_unix()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        Ok(())
    }

    /// Increment and return a task's retry counter.
    pub fn increment_retry(&self, id: &str) -> Result<i64, StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let n = conn.execute(
            "UPDATE tasks SET retry_count = retry_count + 1, updated_at = ?2 WHERE id = ?1",
            params![id, now_unix()],
        )?;
        if n == 0 {
            return Err(StoreError::NotFound(id.to_string()));
        }
        let count: i64 = conn.query_row(
            "SELECT retry_count FROM tasks WHERE id = ?1",
            params![id],
            |r| r.get(0),
        )?;
        Ok(count)
    }

    /// Remove a task from history. Returns whether a row was deleted.
    pub fn remove(&self, id: &str) -> Result<bool, StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let n = conn.execute("DELETE FROM tasks WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    /// Recover tasks that were in flight when the app stopped uncleanly.
    ///
    /// In-flight states (resolving/downloading/retrying/verifying) are reset to
    /// `queued` so they can be safely restarted. Completed, failed, cancelled,
    /// and paused tasks are left untouched — in particular, completed tasks are
    /// never requeued.
    pub fn recover_interrupted(&self) -> Result<usize, StoreError> {
        let conn = self.conn.lock().expect("task store mutex");
        let n = conn.execute(
            "UPDATE tasks SET state = 'queued', updated_at = ?1
             WHERE state IN ('resolving','downloading','retrying','verifying')",
            params![now_unix()],
        )?;
        Ok(n)
    }
}

/// Column list shared by SELECT statements, in the order `row_to_task` expects.
const COLUMNS: &str = "id, source_url, display_url, filename, destination_path, state, \
     total_bytes, downloaded_bytes, mode, etag, last_modified, retry_count, \
     created_at, updated_at, last_error_code";

fn row_to_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadTask> {
    let state_str: String = row.get(5)?;
    let mode_str: String = row.get(8)?;
    Ok(DownloadTask {
        id: row.get(0)?,
        source_url: row.get(1)?,
        display_url: row.get(2)?,
        filename: row.get(3)?,
        destination_path: row.get(4)?,
        state: TaskState::from_db(&state_str).ok_or(rusqlite::Error::InvalidQuery)?,
        total_bytes: row.get(6)?,
        downloaded_bytes: row.get(7)?,
        mode: DownloadMode::from_db(&mode_str).ok_or(rusqlite::Error::InvalidQuery)?,
        etag: row.get(9)?,
        last_modified: row.get(10)?,
        retry_count: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
        last_error_code: row.get(14)?,
    })
}

/// Apply schema migrations based on `PRAGMA user_version`.
fn migrate(conn: &Connection) -> Result<(), StoreError> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version < 1 {
        conn.execute_batch(
            "CREATE TABLE tasks (
                id               TEXT PRIMARY KEY NOT NULL,
                source_url       TEXT NOT NULL,
                display_url      TEXT NOT NULL,
                filename         TEXT NOT NULL,
                destination_path TEXT NOT NULL,
                state            TEXT NOT NULL,
                total_bytes      INTEGER,
                downloaded_bytes INTEGER NOT NULL DEFAULT 0,
                mode             TEXT NOT NULL,
                etag             TEXT,
                last_modified    TEXT,
                retry_count      INTEGER NOT NULL DEFAULT 0,
                created_at       INTEGER NOT NULL,
                updated_at       INTEGER NOT NULL,
                last_error_code  TEXT
             );
             CREATE INDEX idx_tasks_state ON tasks(state);
             CREATE INDEX idx_tasks_created_at ON tasks(created_at);",
        )?;
    }

    // Future migrations: `if version < 2 { ... }`, etc.

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(id: &str) -> DownloadTask {
        DownloadTask::new_queued(
            id,
            "https://cdn.example.com/f.bin?token=SECRET",
            "https://cdn.example.com/f.bin?<redacted>",
            "f.bin",
            "C:/downloads/f.bin",
            DownloadMode::Single,
        )
    }

    #[test]
    fn migrate_sets_version() {
        let store = TaskStore::open_in_memory().unwrap();
        let conn = store.conn.lock().unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
    }

    #[test]
    fn migrate_is_idempotent() {
        // Running migrate twice on the same connection must not error.
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='tasks'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn create_get_and_list() {
        let store = TaskStore::open_in_memory().unwrap();
        store.create(&sample("a")).unwrap();
        store.create(&sample("b")).unwrap();

        let got = store.get("a").unwrap().unwrap();
        assert_eq!(got.id, "a");
        assert_eq!(got.state, TaskState::Queued);
        assert_eq!(got.source_url, "https://cdn.example.com/f.bin?token=SECRET");

        assert_eq!(store.list().unwrap().len(), 2);
        assert!(store.get("missing").unwrap().is_none());
    }

    #[test]
    fn valid_transition_persists() {
        let store = TaskStore::open_in_memory().unwrap();
        store.create(&sample("a")).unwrap();
        store.update_state("a", TaskState::Resolving).unwrap();
        store.update_state("a", TaskState::Downloading).unwrap();
        assert_eq!(
            store.get("a").unwrap().unwrap().state,
            TaskState::Downloading
        );
    }

    #[test]
    fn invalid_transition_rejected() {
        let store = TaskStore::open_in_memory().unwrap();
        store.create(&sample("a")).unwrap();
        let err = store.update_state("a", TaskState::Completed).unwrap_err();
        assert!(matches!(err, StoreError::InvalidTransition { .. }));
        // State unchanged.
        assert_eq!(store.get("a").unwrap().unwrap().state, TaskState::Queued);
    }

    #[test]
    fn update_state_on_missing_is_not_found() {
        let store = TaskStore::open_in_memory().unwrap();
        let err = store
            .update_state("nope", TaskState::Resolving)
            .unwrap_err();
        assert!(matches!(err, StoreError::NotFound(_)));
    }

    #[test]
    fn progress_error_retry_and_remove() {
        let store = TaskStore::open_in_memory().unwrap();
        store.create(&sample("a")).unwrap();

        store.update_progress("a", 1234, Some(5000)).unwrap();
        let t = store.get("a").unwrap().unwrap();
        assert_eq!(t.downloaded_bytes, 1234);
        assert_eq!(t.total_bytes, Some(5000));

        store.set_error("a", "NETWORK_ERROR").unwrap();
        assert_eq!(
            store.get("a").unwrap().unwrap().last_error_code.as_deref(),
            Some("NETWORK_ERROR")
        );

        assert_eq!(store.increment_retry("a").unwrap(), 1);
        assert_eq!(store.increment_retry("a").unwrap(), 2);

        assert!(store.remove("a").unwrap());
        assert!(!store.remove("a").unwrap());
        assert!(store.get("a").unwrap().is_none());
    }

    #[test]
    fn recovery_resets_in_flight_but_not_terminal() {
        let store = TaskStore::open_in_memory().unwrap();

        // An interrupted downloading task.
        store.create(&sample("dl")).unwrap();
        store.update_state("dl", TaskState::Resolving).unwrap();
        store.update_state("dl", TaskState::Downloading).unwrap();

        // A completed task that must NOT be requeued.
        store.create(&sample("done")).unwrap();
        store.update_state("done", TaskState::Resolving).unwrap();
        store.update_state("done", TaskState::Downloading).unwrap();
        store.update_state("done", TaskState::Verifying).unwrap();
        store.update_state("done", TaskState::Completed).unwrap();

        // A paused task that should stay paused.
        store.create(&sample("pause")).unwrap();
        store.update_state("pause", TaskState::Resolving).unwrap();
        store.update_state("pause", TaskState::Downloading).unwrap();
        store.update_state("pause", TaskState::Paused).unwrap();

        let recovered = store.recover_interrupted().unwrap();
        assert_eq!(recovered, 1);

        assert_eq!(store.get("dl").unwrap().unwrap().state, TaskState::Queued);
        assert_eq!(
            store.get("done").unwrap().unwrap().state,
            TaskState::Completed
        );
        assert_eq!(
            store.get("pause").unwrap().unwrap().state,
            TaskState::Paused
        );
    }

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tasks.db");
        {
            let store = TaskStore::open(&path).unwrap();
            store.create(&sample("keep")).unwrap();
        }
        // Reopen: the task survives.
        let store = TaskStore::open(&path).unwrap();
        assert_eq!(store.get("keep").unwrap().unwrap().id, "keep");
    }
}
