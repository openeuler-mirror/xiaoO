use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{Connection, Row};

use super::super::storage_trait::{SpanStorage, TraceSummary};
use crate::{MoiraiError, Result, Span};

const SPAN_PREFIX_SUGGESTION_LIMIT: usize = 5;

#[derive(Clone)]
pub struct SqliteStorage {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteStorage {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS spans (
                span_id TEXT PRIMARY KEY,
                trace_id TEXT NOT NULL,
                parent_span_id TEXT,
                span_type TEXT NOT NULL,
                start_time INTEGER NOT NULL,
                last_updated_at INTEGER NOT NULL,
                end_time INTEGER,
                extras TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_trace_id ON spans(trace_id);
            CREATE INDEX IF NOT EXISTS idx_parent_span_id ON spans(parent_span_id);
            CREATE INDEX IF NOT EXISTS idx_created_at ON spans(created_at DESC);
            "#,
        )?;
        let alter_result = conn.execute(
            "ALTER TABLE spans ADD COLUMN last_updated_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        if let Err(error) = alter_result {
            if !error.to_string().contains("duplicate column name") {
                return Err(error.into());
            }
        }
        Ok(())
    }

    fn span_from_row(row: &Row) -> Result<Span> {
        let span_type: String = row.get(3)?;
        let extras_str: String = row.get(7)?;
        let extras: serde_json::Value = serde_json::from_str(&extras_str)?;

        Ok(Span {
            span_id: row.get(0)?,
            trace_id: row.get(1)?,
            parent_span_id: row.get(2)?,
            span_type,
            start_time: row.get(4)?,
            last_updated_at: row.get(5)?,
            end_time: row.get(6)?,
            extras,
            created_at: row.get(8)?,
        })
    }
}

#[async_trait]
impl SpanStorage for SqliteStorage {
    async fn insert_span(&self, span: &Span) -> Result<()> {
        let conn = self.conn.clone();
        let span = span.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let extras_str = serde_json::to_string(&span.extras)?;

            conn.execute(
                "INSERT INTO spans (span_id, trace_id, parent_span_id, span_type, start_time, last_updated_at, end_time, extras, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                rusqlite::params![
                    span.span_id,
                    span.trace_id,
                    span.parent_span_id,
                    span.span_type,
                    span.start_time,
                    span.last_updated_at,
                    span.end_time,
                    extras_str,
                    span.created_at,
                ],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn update_span_end(
        &self,
        span_id: &str,
        end_time: i64,
        last_updated_at: i64,
    ) -> Result<()> {
        let conn = self.conn.clone();
        let span_id = span_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let rows_affected = conn.execute(
                "UPDATE spans SET end_time = ?1, last_updated_at = ?2 WHERE span_id = ?3",
                rusqlite::params![end_time, last_updated_at, span_id],
            )?;

            if rows_affected == 0 {
                return Err(MoiraiError::NotFound(format!(
                    "Span not found: {}",
                    span_id
                )));
            }
            Ok(())
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn update_span_extras(
        &self,
        span_id: &str,
        extras: serde_json::Value,
        last_updated_at: i64,
        end_time: Option<i64>,
    ) -> Result<()> {
        let conn = self.conn.clone();
        let span_id = span_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                "SELECT extras FROM spans WHERE span_id = ?1",
            )?;

            let current_extras_str: String = stmt.query_row(
                rusqlite::params![&span_id],
                |row| row.get(0),
            ).map_err(|_| MoiraiError::NotFound(format!(
                "Span not found: {}",
                span_id
            )))?;

            let mut current_extras: serde_json::Value = serde_json::from_str(&current_extras_str)?;

            if let (serde_json::Value::Object(ref mut current_obj), serde_json::Value::Object(new_obj)) = (&mut current_extras, &extras) {
                for (key, value) in new_obj {
                    current_obj.insert(key.clone(), value.clone());
                }
            }

            let merged_extras_str = serde_json::to_string(&current_extras)?;

            if let Some(end_time_val) = end_time {
                conn.execute(
                    "UPDATE spans SET extras = ?1, last_updated_at = ?2, end_time = ?3 WHERE span_id = ?4",
                    rusqlite::params![merged_extras_str, last_updated_at, end_time_val, span_id],
                )?;
            } else {
                conn.execute(
                    "UPDATE spans SET extras = ?1, last_updated_at = ?2 WHERE span_id = ?3",
                    rusqlite::params![merged_extras_str, last_updated_at, span_id],
                )?;
            }

            Ok(())
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn get_span(&self, span_id: &str) -> Result<Option<Span>> {
        let conn = self.conn.clone();
        let span_id = span_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                "SELECT span_id, trace_id, parent_span_id, span_type, start_time, last_updated_at, end_time, extras, created_at
                 FROM spans WHERE span_id = ?1",
            )?;

            let mut rows = stmt.query(rusqlite::params![span_id])?;

            match rows.next()? {
                Some(row) => Ok(Some(SqliteStorage::span_from_row(row)?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn get_trace_spans(&self, trace_id: &str) -> Result<Vec<Span>> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                "SELECT span_id, trace_id, parent_span_id, span_type, start_time, last_updated_at, end_time, extras, created_at
                 FROM spans WHERE trace_id = ?1 ORDER BY created_at ASC",
            )?;

            let rows = stmt.query_map(rusqlite::params![trace_id], |row| {
                Ok(SqliteStorage::span_from_row(row))
            })?;

            let mut spans = Vec::new();
            for row_result in rows {
                spans.push(row_result??);
            }
            Ok(spans)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn list_traces(&self, limit: usize) -> Result<Vec<TraceSummary>> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                r#"
                SELECT
                    s.trace_id,
                    COUNT(*) as span_count,
                    MIN(s.start_time) as start_time,
                    (SELECT end_time FROM spans
                     WHERE trace_id = s.trace_id AND span_type = 'END'
                     LIMIT 1) as end_time,
                    (SELECT span_type FROM spans WHERE trace_id = s.trace_id AND parent_span_id IS NULL LIMIT 1) as root_span_type
                FROM spans s
                GROUP BY s.trace_id
                ORDER BY start_time DESC
                LIMIT ?1
                "#,
            )?;

            let rows = stmt.query_map(rusqlite::params![limit], |row| {
                Ok(TraceSummary {
                    trace_id: row.get(0)?,
                    span_count: row.get(1)?,
                    start_time: row.get(2)?,
                    end_time: row.get(3)?,
                    root_span_type: row.get(4)?,
                })
            })?;

            let mut summaries = Vec::new();
            for row_result in rows {
                summaries.push(row_result?);
            }
            Ok(summaries)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn list_alive_traces(&self, limit: usize) -> Result<Vec<TraceSummary>> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                r#"
                SELECT
                    s.trace_id,
                    COUNT(*) as span_count,
                    MIN(s.start_time) as start_time,
                    MAX(s.end_time) as end_time,
                    (SELECT span_type FROM spans WHERE trace_id = s.trace_id AND parent_span_id IS NULL LIMIT 1) as root_span_type
                FROM spans s
                WHERE s.trace_id NOT IN (
                    SELECT DISTINCT trace_id FROM spans WHERE span_type = 'END'
                )
                GROUP BY s.trace_id
                ORDER BY start_time DESC
                LIMIT ?1
                "#,
            )?;

            let rows = stmt.query_map(rusqlite::params![limit], |row| {
                Ok(TraceSummary {
                    trace_id: row.get(0)?,
                    span_count: row.get(1)?,
                    start_time: row.get(2)?,
                    end_time: row.get(3)?,
                    root_span_type: row.get(4)?,
                })
            })?;

            let mut summaries = Vec::new();
            for row_result in rows {
                summaries.push(row_result?);
            }
            Ok(summaries)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn get_trace_by_prefix(&self, prefix: &str) -> Result<Option<String>> {
        let conn = self.conn.clone();
        let prefix = prefix.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                "SELECT DISTINCT trace_id FROM spans WHERE trace_id LIKE ?1 ORDER BY trace_id LIMIT 10",
            )?;

            let pattern = format!("{}%", prefix);
            let rows: Vec<String> = stmt
                .query_map(rusqlite::params![pattern], |row| row.get(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            match rows.len() {
                0 => Ok(None),
                1 => Ok(Some(rows.into_iter().next().unwrap())),
                _ => {
                    let matches: Vec<String> = rows.iter().map(|id| id[..12].to_string()).collect();
                    Err(MoiraiError::InvalidState(format!(
                        "Multiple traces match prefix '{}': {}",
                        prefix,
                        matches.join(", ")
                    )))
                }
            }
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn get_span_by_prefix(&self, prefix: &str) -> Result<Option<String>> {
        let conn = self.conn.clone();
        let prefix = prefix.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let pattern = format!("{}%", prefix);

            let total_matches: usize = conn.query_row(
                "SELECT COUNT(*) FROM spans WHERE span_id LIKE ?1",
                rusqlite::params![&pattern],
                |row| row.get(0),
            )?;

            if total_matches == 0 {
                return Ok(None);
            }

            let mut stmt = conn.prepare(
                "SELECT span_id FROM spans WHERE span_id LIKE ?1 ORDER BY span_id LIMIT ?2",
            )?;

            let matches: Vec<String> = stmt
                .query_map(
                    rusqlite::params![&pattern, SPAN_PREFIX_SUGGESTION_LIMIT],
                    |row| row.get(0),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            if total_matches == 1 {
                return Ok(matches.into_iter().next());
            }

            Err(MoiraiError::InvalidState(format!(
                "Multiple spans match prefix '{}': {} ({} total matches)",
                prefix,
                matches.join(", "),
                total_matches
            )))
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn get_last_span_id(&self, trace_id: &str) -> Result<Option<String>> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let mut stmt = conn.prepare(
                "SELECT span_id FROM spans WHERE trace_id = ?1 ORDER BY start_time DESC LIMIT 1",
            )?;

            let result = stmt
                .query_map(rusqlite::params![trace_id], |row| row.get(0))?
                .next();

            match result {
                Some(r) => Ok(Some(r?)),
                None => Ok(None),
            }
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    async fn count_spans(&self, trace_id: &str) -> Result<usize> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let count: usize = conn.query_row(
                "SELECT COUNT(*) FROM spans WHERE trace_id = ?1",
                rusqlite::params![trace_id],
                |row| row.get(0),
            )?;

            Ok(count)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }
}

impl SqliteStorage {
    pub async fn delete_old_spans(&self, cutoff_timestamp_ms: i64) -> Result<usize> {
        let conn = self.conn.clone();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let deleted = conn.execute(
                "DELETE FROM spans WHERE created_at < ?1",
                rusqlite::params![cutoff_timestamp_ms],
            )?;

            Ok(deleted)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    pub async fn delete_trace(&self, trace_id: &str) -> Result<usize> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let deleted = conn.execute(
                "DELETE FROM spans WHERE trace_id = ?1",
                rusqlite::params![trace_id],
            )?;

            Ok(deleted)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }
}
