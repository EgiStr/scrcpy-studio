//! Run history and the audit log, in SQLite under the app data directory, so jobs and
//! their verdicts survive a restart. Evidence files stay on disk; rows only point at them.

use crate::error::{AppError, Result};
use crate::qa::Job;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct Store {
    conn: Connection,
}

/// One audit-log entry.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub at: u64,
    pub kind: String,
    pub job_id: Option<u32>,
    pub run_id: Option<String>,
    pub detail: String,
}

fn db(e: rusqlite::Error) -> AppError {
    AppError::Io(format!("history database: {e}"))
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path).map_err(db)?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             CREATE TABLE IF NOT EXISTS jobs (
                 id INTEGER PRIMARY KEY,
                 run_id TEXT NOT NULL,
                 scenario TEXT NOT NULL,
                 status TEXT NOT NULL,
                 serial TEXT,
                 queued_at INTEGER NOT NULL,
                 finished_at INTEGER,
                 data TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS jobs_scenario ON jobs (scenario);
             CREATE TABLE IF NOT EXISTS events (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 at INTEGER NOT NULL,
                 kind TEXT NOT NULL,
                 job_id INTEGER,
                 run_id TEXT,
                 detail TEXT NOT NULL
             );",
        )
        .map_err(db)?;
        Ok(Self { conn })
    }

    pub fn save_job(&self, job: &Job) -> Result<()> {
        let data = serde_json::to_string(job).map_err(|e| AppError::Io(e.to_string()))?;
        self.conn
            .execute(
                "INSERT INTO jobs (id, run_id, scenario, status, serial, queued_at, finished_at, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT (id) DO UPDATE SET status = ?4, serial = ?5, finished_at = ?7, data = ?8",
                params![job.id, job.run_id, job.scenario, job.status.as_str(), job.serial, job.queued_at, job.finished_at, data],
            )
            .map_err(db)?;
        Ok(())
    }

    /// The newest `limit` jobs, oldest first.
    pub fn jobs(&self, limit: u32) -> Result<Vec<Job>> {
        let mut stmt = self.conn.prepare("SELECT data FROM jobs ORDER BY id DESC LIMIT ?1").map_err(db)?;
        let rows = stmt.query_map([limit], |r| r.get::<_, String>(0)).map_err(db)?;
        let mut jobs: Vec<Job> = rows.filter_map(|r| r.ok()).filter_map(|t| serde_json::from_str(&t).ok()).collect();
        jobs.reverse();
        Ok(jobs)
    }

    pub fn max_job_id(&self) -> Result<u32> {
        let id: Option<u32> = self.conn.query_row("SELECT MAX(id) FROM jobs", [], |r| r.get(0)).optional().map_err(db)?.flatten();
        Ok(id.unwrap_or(0))
    }

    pub fn event(&self, at: u64, kind: &str, job_id: Option<u32>, run_id: Option<&str>, detail: &str) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO events (at, kind, job_id, run_id, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![at, kind, job_id, run_id, detail],
            )
            .map_err(db)?;
        Ok(())
    }

    /// The newest `limit` audit entries, newest first.
    pub fn events(&self, limit: u32) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT at, kind, job_id, run_id, detail FROM events ORDER BY id DESC LIMIT ?1")
            .map_err(db)?;
        let rows = stmt
            .query_map([limit], |r| Ok(Event { at: r.get(0)?, kind: r.get(1)?, job_id: r.get(2)?, run_id: r.get(3)?, detail: r.get(4)? }))
            .map_err(db)?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}
