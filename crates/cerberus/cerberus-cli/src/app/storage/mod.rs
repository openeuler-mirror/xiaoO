//! Storage layer for execution history.

use crate::app::error::CliError;
use cerberus_core::ExecResult;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// Execution record for storage.
#[derive(Debug, Serialize, Deserialize)]
pub struct ExecutionRecord {
    /// Unique ID.
    pub id: i64,
    /// Command that was executed.
    pub command: String,
    /// Exit code.
    pub exit_code: i32,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the command timed out.
    pub timed_out: bool,
    /// Timestamp (ISO 8601).
    pub timestamp: String,
}

/// SQLite storage backend.
pub struct Storage {
    conn: Connection,
}

impl Storage {
    /// Open the default storage database.
    pub fn open_default() -> Result<Self, CliError> {
        let db_path = Self::get_db_path()?;
        Self::open_at(&db_path)
    }

    /// Open storage at a specific path (for testing).
    pub fn open_at(db_path: &std::path::Path) -> Result<Self, CliError> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(db_path)?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS executions (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                command TEXT NOT NULL,
                exit_code INTEGER NOT NULL,
                duration_ms INTEGER NOT NULL,
                timed_out INTEGER NOT NULL,
                timestamp TEXT NOT NULL
            )",
            [],
        )?;

        Ok(Self { conn })
    }

    /// Record an execution.
    pub fn record_execution(
        &self,
        command: &str,
        result: &ExecResult,
        duration: Duration,
    ) -> Result<i64, CliError> {
        let timestamp = chrono::Utc::now().to_rfc3339();
        let duration_ms = duration.as_millis() as u64;

        self.conn.execute(
            "INSERT INTO executions (command, exit_code, duration_ms, timed_out, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                command,
                result.exit_code,
                duration_ms as i64,
                result.metadata.timed_out as i64,
                &timestamp
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get execution records.
    pub fn get_records(
        &self,
        since: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ExecutionRecord>, CliError> {
        let mut query =
            "SELECT id, command, exit_code, duration_ms, timed_out, timestamp FROM executions"
                .to_string();
        let mut conditions = Vec::new();

        if let Some(since_str) = since {
            if let Some(timestamp) = parse_since_param(since_str) {
                conditions.push(format!("timestamp >= '{}'", timestamp));
            }
        }

        if !conditions.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&conditions.join(" AND "));
        }

        query.push_str(" ORDER BY timestamp DESC");

        if let Some(limit) = limit {
            query.push_str(&format!(" LIMIT {}", limit));
        }

        let mut stmt = self.conn.prepare(&query)?;
        let records = stmt
            .query_map([], |row| {
                Ok(ExecutionRecord {
                    id: row.get(0)?,
                    command: row.get(1)?,
                    exit_code: row.get(2)?,
                    duration_ms: row.get::<_, i64>(3)? as u64,
                    timed_out: row.get::<_, i64>(4)? != 0,
                    timestamp: row.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(records)
    }

    fn get_db_path() -> Result<PathBuf, CliError> {
        let data_dir = dirs::data_local_dir()
            .or_else(dirs::data_dir)
            .ok_or_else(|| CliError::StorageError("Cannot determine data directory".to_string()))?;

        Ok(data_dir.join("cerberus").join("cerberus.db"))
    }
}

fn parse_since_param(since: &str) -> Option<String> {
    let now = chrono::Utc::now();

    let (value, unit) = since.split_at(since.len().saturating_sub(1));
    let num: i64 = value.parse().ok()?;

    let datetime = match unit {
        "d" => now - chrono::Duration::days(num),
        "h" => now - chrono::Duration::hours(num),
        "w" => now - chrono::Duration::weeks(num),
        _ => return None,
    };

    Some(datetime.to_rfc3339())
}
