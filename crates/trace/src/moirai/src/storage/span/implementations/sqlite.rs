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

#[derive(Debug, Clone, PartialEq, Eq)]
struct TraceSegment {
    trace_id: String,
    start_time: i64,
    end_time: Option<i64>,
    session_id: Option<String>,
    agent_id: Option<String>,
    end_message: Option<String>,
}

impl SqliteStorage {
    pub fn new(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::apply_pragmas(&conn)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Apply SQLite connection pragmas so concurrent connections writing to the
    /// same `trace.db` (e.g. the TUI and the daemon, or the `moirai` inspection
    /// CLI) don't immediately fail with `SQLITE_BUSY` ("database is locked"):
    ///
    /// * `journal_mode = WAL` — readers and one writer can run concurrently,
    ///   avoiding rollback-journal exclusive locks.
    /// * `busy_timeout = 5000` — a writer that hits a lock waits/retries for up
    ///   to 5s instead of returning `SQLITE_BUSY` right away (the default is 0,
    ///   which fails instantly and is the root cause of the
    ///   "moirai trace finalization failed: Storage error: database is locked"
    ///   messages).
    /// * `synchronous = NORMAL` — safe under WAL and much faster than the
    ///   default `FULL`.
    fn apply_pragmas(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA busy_timeout = 5000;
            PRAGMA synchronous = NORMAL;
            "#,
        )?;
        Ok(())
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

    fn trace_segment_from_row(row: &Row) -> Result<TraceSegment> {
        Ok(TraceSegment {
            trace_id: row.get(0)?,
            start_time: row.get(1)?,
            end_time: row.get(2)?,
            session_id: row.get(3)?,
            agent_id: row.get(4)?,
            end_message: row.get(5)?,
        })
    }

    fn load_trace_segment(conn: &Connection, trace_id: &str) -> Result<Option<TraceSegment>> {
        let mut stmt = conn.prepare(
            r#"
            SELECT
                s.trace_id,
                MIN(s.start_time) AS start_time,
                COALESCE(
                    (SELECT end_time
                     FROM spans end_spans
                     WHERE end_spans.trace_id = s.trace_id AND end_spans.span_type = 'END'
                     LIMIT 1),
                    MAX(COALESCE(s.end_time, s.start_time))
                ) AS end_time,
                (SELECT json_extract(meta.extras, '$.session_id')
                 FROM spans meta
                 WHERE meta.trace_id = s.trace_id
                   AND json_extract(meta.extras, '$.session_id') IS NOT NULL
                 LIMIT 1) AS session_id,
                (SELECT json_extract(meta.extras, '$.agent_id')
                 FROM spans meta
                 WHERE meta.trace_id = s.trace_id
                   AND json_extract(meta.extras, '$.agent_id') IS NOT NULL
                 LIMIT 1) AS agent_id,
                (SELECT json_extract(meta.extras, '$.message')
                 FROM spans meta
                 WHERE meta.trace_id = s.trace_id AND meta.span_type = 'END'
                 LIMIT 1) AS end_message
            FROM spans s
            WHERE s.trace_id = ?1
            GROUP BY s.trace_id
            "#,
        )?;

        let mut rows = stmt.query(rusqlite::params![trace_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::trace_segment_from_row(row)?)),
            None => Ok(None),
        }
    }

    fn find_previous_trace_segment(
        conn: &Connection,
        session_id: &str,
        agent_id: &str,
        before_start_time: i64,
    ) -> Result<Option<TraceSegment>> {
        let mut stmt = conn.prepare(
            r#"
            SELECT trace_id, start_time, end_time, session_id, agent_id, end_message
            FROM (
                SELECT
                    s.trace_id AS trace_id,
                    MIN(s.start_time) AS start_time,
                    COALESCE(
                        (SELECT end_spans.end_time
                         FROM spans end_spans
                         WHERE end_spans.trace_id = s.trace_id AND end_spans.span_type = 'END'
                         LIMIT 1),
                        MAX(COALESCE(s.end_time, s.start_time))
                    ) AS end_time,
                    ?1 AS session_id,
                    ?2 AS agent_id,
                    (SELECT json_extract(end_spans.extras, '$.message')
                     FROM spans end_spans
                     WHERE end_spans.trace_id = s.trace_id AND end_spans.span_type = 'END'
                     LIMIT 1) AS end_message
                FROM spans s
                WHERE EXISTS (
                    SELECT 1
                    FROM spans meta
                    WHERE meta.trace_id = s.trace_id
                      AND json_extract(meta.extras, '$.session_id') = ?1
                      AND json_extract(meta.extras, '$.agent_id') = ?2
                )
                GROUP BY s.trace_id
            )
            WHERE start_time < ?3
            ORDER BY start_time DESC
            LIMIT 1
            "#,
        )?;

        let mut rows = stmt.query(rusqlite::params![session_id, agent_id, before_start_time])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::trace_segment_from_row(row)?)),
            None => Ok(None),
        }
    }

    fn find_next_trace_segment(
        conn: &Connection,
        session_id: &str,
        agent_id: &str,
        after_end_time: i64,
    ) -> Result<Option<TraceSegment>> {
        let mut stmt = conn.prepare(
            r#"
            SELECT trace_id, start_time, end_time, session_id, agent_id, end_message
            FROM (
                SELECT
                    s.trace_id AS trace_id,
                    MIN(s.start_time) AS start_time,
                    COALESCE(
                        (SELECT end_spans.end_time
                         FROM spans end_spans
                         WHERE end_spans.trace_id = s.trace_id AND end_spans.span_type = 'END'
                         LIMIT 1),
                        MAX(COALESCE(s.end_time, s.start_time))
                    ) AS end_time,
                    ?1 AS session_id,
                    ?2 AS agent_id,
                    (SELECT json_extract(end_spans.extras, '$.message')
                     FROM spans end_spans
                     WHERE end_spans.trace_id = s.trace_id AND end_spans.span_type = 'END'
                     LIMIT 1) AS end_message
                FROM spans s
                WHERE EXISTS (
                    SELECT 1
                    FROM spans meta
                    WHERE meta.trace_id = s.trace_id
                      AND json_extract(meta.extras, '$.session_id') = ?1
                      AND json_extract(meta.extras, '$.agent_id') = ?2
                )
                GROUP BY s.trace_id
            )
            WHERE start_time > ?3
            ORDER BY start_time ASC
            LIMIT 1
            "#,
        )?;

        let mut rows = stmt.query(rusqlite::params![session_id, agent_id, after_end_time])?;
        match rows.next()? {
            Some(row) => Ok(Some(Self::trace_segment_from_row(row)?)),
            None => Ok(None),
        }
    }

    fn load_trace_spans(conn: &Connection, trace_id: &str) -> Result<Vec<Span>> {
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
    pub async fn get_suspend_resume_trace_chain(&self, trace_id: &str) -> Result<Vec<String>> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let current = Self::load_trace_segment(&conn, &trace_id)?
                .ok_or_else(|| MoiraiError::NotFound(format!("Trace not found: {}", trace_id)))?;
            let Some(session_id) = current.session_id.clone() else {
                return Ok(vec![current.trace_id]);
            };
            let Some(agent_id) = current.agent_id.clone() else {
                return Ok(vec![current.trace_id]);
            };

            let mut previous_ids = Vec::new();
            let mut backward_cursor = current.clone();
            while let Some(previous) = Self::find_previous_trace_segment(
                &conn,
                &session_id,
                &agent_id,
                backward_cursor.start_time,
            )? {
                if previous.end_message.as_deref() != Some("suspended") {
                    break;
                }
                previous_ids.push(previous.trace_id.clone());
                backward_cursor = previous;
            }
            previous_ids.reverse();

            let mut chain = previous_ids;
            chain.push(current.trace_id.clone());

            let mut forward_cursor = current;
            while forward_cursor.end_message.as_deref() == Some("suspended") {
                let boundary = forward_cursor.end_time.unwrap_or(forward_cursor.start_time);
                let Some(next) =
                    Self::find_next_trace_segment(&conn, &session_id, &agent_id, boundary)?
                else {
                    break;
                };
                if chain.last() != Some(&next.trace_id) {
                    chain.push(next.trace_id.clone());
                }
                forward_cursor = next;
            }

            Ok(chain)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    pub async fn get_trace_spans_with_related_segments(&self, trace_id: &str) -> Result<Vec<Span>> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let chain = {
                let current = Self::load_trace_segment(&conn, &trace_id)?.ok_or_else(|| {
                    MoiraiError::NotFound(format!("Trace not found: {}", trace_id))
                })?;
                let Some(session_id) = current.session_id.clone() else {
                    return Self::load_trace_spans(&conn, &trace_id);
                };
                let Some(agent_id) = current.agent_id.clone() else {
                    return Self::load_trace_spans(&conn, &trace_id);
                };

                let mut previous_ids = Vec::new();
                let mut backward_cursor = current.clone();
                while let Some(previous) = Self::find_previous_trace_segment(
                    &conn,
                    &session_id,
                    &agent_id,
                    backward_cursor.start_time,
                )? {
                    if previous.end_message.as_deref() != Some("suspended") {
                        break;
                    }
                    previous_ids.push(previous.trace_id.clone());
                    backward_cursor = previous;
                }
                previous_ids.reverse();

                let mut chain = previous_ids;
                chain.push(current.trace_id.clone());

                let mut forward_cursor = current;
                while forward_cursor.end_message.as_deref() == Some("suspended") {
                    let boundary = forward_cursor.end_time.unwrap_or(forward_cursor.start_time);
                    let Some(next) =
                        Self::find_next_trace_segment(&conn, &session_id, &agent_id, boundary)?
                    else {
                        break;
                    };
                    if chain.last() != Some(&next.trace_id) {
                        chain.push(next.trace_id.clone());
                    }
                    forward_cursor = next;
                }

                chain
            };

            let mut spans = Vec::new();
            for segment_trace_id in chain {
                spans.extend(Self::load_trace_spans(&conn, &segment_trace_id)?);
            }
            spans.sort_by(|left, right| {
                left.start_time
                    .cmp(&right.start_time)
                    .then(left.created_at.cmp(&right.created_at))
                    .then(left.span_id.cmp(&right.span_id))
            });
            Ok(spans)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

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

#[cfg(test)]
mod tests {
    use super::SqliteStorage;
    use crate::{Span, SpanStorage};

    fn span(
        span_id: &str,
        trace_id: &str,
        parent_span_id: Option<&str>,
        span_type: &str,
        start_time: i64,
        end_time: Option<i64>,
        extras: serde_json::Value,
    ) -> Span {
        Span {
            span_id: span_id.to_string(),
            trace_id: trace_id.to_string(),
            parent_span_id: parent_span_id.map(ToString::to_string),
            span_type: span_type.to_string(),
            start_time,
            last_updated_at: end_time.unwrap_or(start_time),
            end_time,
            extras,
            created_at: start_time,
        }
    }

    #[test]
    fn pragmas_are_applied_on_open() {
        // Regression test for "database is locked": opening a SqliteStorage on a
        // real file must enable WAL + a non-zero busy_timeout so concurrent
        // connections don't fail instantly with SQLITE_BUSY.
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("trace.db");
        let storage = SqliteStorage::new(db_path.to_str().unwrap()).expect("open storage");

        let conn = storage.conn.lock().expect("lock conn");
        let journal: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read journal_mode");
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("read busy_timeout");
        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("read synchronous");
        drop(conn);

        assert_eq!(
            journal.to_lowercase(),
            "wal",
            "journal_mode must be WAL to allow concurrent readers + a writer"
        );
        assert_eq!(
            busy_timeout, 5000,
            "busy_timeout must be non-zero so writers wait instead of failing SQLITE_BUSY"
        );
        assert_eq!(
            synchronous, 1,
            "synchronous must be NORMAL (1) under WAL; OFF (0) is unsafe, FULL (2) undoes the perf win"
        );
    }

    #[tokio::test]
    async fn concurrent_writes_do_not_fail_with_database_locked() {
        // End-to-end regression for the "database is locked" root cause: two
        // separate `SqliteStorage` instances (e.g. the TUI and the daemon)
        // opening the same `trace.db` file must not return SQLITE_BUSY
        // immediately. With WAL + busy_timeout from `apply_pragmas`, the second
        // writer waits for the first to commit and then succeeds; with the
        // default busy_timeout=0, B would fail instantly with SQLITE_BUSY.
        use std::time::Duration;

        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("trace.db");
        let path = db_path.to_str().unwrap().to_string();

        let storage_a = SqliteStorage::new(&path).expect("open storage A");
        let storage_b = SqliteStorage::new(&path).expect("open storage B");

        // Hold an uncommitted write transaction on A so B's concurrent insert
        // hits a locked database. We hold the lock briefly, then commit; by
        // then B's insert is waiting. With busy_timeout=5000ms it waits and
        // succeeds; with the default 0 it would return SQLITE_BUSY immediately.
        let conn_a = storage_a.conn.clone();
        let holder = tokio::task::spawn_blocking(move || {
            let conn = conn_a.lock().expect("lock A");
            conn.execute("BEGIN IMMEDIATE", [])
                .expect("begin immediate");
            conn.execute(
                "INSERT INTO spans (span_id, trace_id, parent_span_id, span_type, \
                 start_time, last_updated_at, end_time, extras, created_at) \
                 VALUES ('holder', 'trace-x', NULL, 'USER', 1, 1, NULL, '{}', 1)",
                [],
            )
            .expect("insert holder");
            std::thread::sleep(Duration::from_millis(500));
            conn.execute("COMMIT", []).expect("commit");
        });

        let insert_b = tokio::spawn(async move {
            storage_b
                .insert_span(&span(
                    "writer",
                    "trace-x",
                    None,
                    "TOOL_CALL",
                    2,
                    Some(3),
                    serde_json::json!({}),
                ))
                .await
        });

        holder.await.expect("holder join error");
        let result = tokio::time::timeout(Duration::from_secs(10), insert_b)
            .await
            .expect("B did not finish within 10s")
            .expect("B join error");
        result.expect("B must succeed, not SQLITE_BUSY");
    }

    #[tokio::test]
    async fn suspend_resume_chain_includes_previous_and_next_segments() {
        let storage = SqliteStorage::new(":memory:").unwrap();

        storage
            .insert_span(&span(
                "root-a",
                "trace-a",
                None,
                "USER",
                100,
                None,
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        storage
            .insert_span(&span(
                "tool-a",
                "trace-a",
                Some("root-a"),
                "TOOL_CALL",
                110,
                Some(120),
                serde_json::json!({
                    "session_id": "session-1",
                    "agent_id": "main",
                    "tool_name": "spawn_subagent"
                }),
            ))
            .await
            .unwrap();
        storage
            .insert_span(&span(
                "end-a",
                "trace-a",
                Some("root-a"),
                "END",
                130,
                Some(130),
                serde_json::json!({
                    "message": "suspended"
                }),
            ))
            .await
            .unwrap();

        storage
            .insert_span(&span(
                "root-b",
                "trace-b",
                None,
                "USER",
                200,
                None,
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        storage
            .insert_span(&span(
                "tool-b",
                "trace-b",
                Some("root-b"),
                "TOOL_CALL",
                210,
                Some(220),
                serde_json::json!({
                    "session_id": "session-1",
                    "agent_id": "main",
                    "tool_name": "join_subagent"
                }),
            ))
            .await
            .unwrap();
        storage
            .insert_span(&span(
                "end-b",
                "trace-b",
                Some("root-b"),
                "END",
                230,
                Some(230),
                serde_json::json!({}),
            ))
            .await
            .unwrap();

        storage
            .insert_span(&span(
                "root-c",
                "trace-c",
                None,
                "USER",
                300,
                None,
                serde_json::json!({}),
            ))
            .await
            .unwrap();
        storage
            .insert_span(&span(
                "tool-c",
                "trace-c",
                Some("root-c"),
                "TOOL_CALL",
                310,
                Some(320),
                serde_json::json!({
                    "session_id": "session-1",
                    "agent_id": "main",
                    "tool_name": "bash"
                }),
            ))
            .await
            .unwrap();
        storage
            .insert_span(&span(
                "end-c",
                "trace-c",
                Some("root-c"),
                "END",
                330,
                Some(330),
                serde_json::json!({}),
            ))
            .await
            .unwrap();

        let chain_from_first = storage
            .get_suspend_resume_trace_chain("trace-a")
            .await
            .unwrap();
        assert_eq!(
            chain_from_first,
            vec!["trace-a".to_string(), "trace-b".to_string()]
        );

        let chain_from_second = storage
            .get_suspend_resume_trace_chain("trace-b")
            .await
            .unwrap();
        assert_eq!(
            chain_from_second,
            vec!["trace-a".to_string(), "trace-b".to_string()]
        );

        let spans = storage
            .get_trace_spans_with_related_segments("trace-b")
            .await
            .unwrap();
        let tool_names = spans
            .iter()
            .filter_map(|span| {
                span.extras
                    .get("tool_name")
                    .and_then(|value| value.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(tool_names, vec!["spawn_subagent", "join_subagent"]);
    }
}
