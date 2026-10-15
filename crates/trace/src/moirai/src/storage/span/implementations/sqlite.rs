use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior};

use super::request_store;

use super::super::storage_trait::{SpanStorage, TraceSummary};
use crate::{MoiraiError, Result, Span};

const SPAN_PREFIX_SUGGESTION_LIMIT: usize = 5;

/// Only LLM_CALL spans may materialize `trace_tasks` rows: the
/// `input_preview` extra is an LLM_CALL-only contract.
const LLM_CALL_SPAN_TYPE: &str = "LLM_CALL";

/// `PRAGMA user_version` marker for one-time migrations; v1 = `trace_tasks`
/// backfill. Bump and chain new steps in `run_migrations` for future ones.
const TRACE_TASKS_MIGRATION_VERSION: i64 = 1;

/// `task_description` projection for trace-list queries, backed by the
/// write-time-materialized `trace_tasks` table.
const TRACE_TASK_DESCRIPTION_SQL: &str =
    "(SELECT tt.task_description FROM trace_tasks tt WHERE tt.trace_id = s.trace_id) as task_description";

/// Cap for materialized task descriptions; mirrors llm-client's
/// `INPUT_PREVIEW_MAX_CHARS` (keep in sync manually).
const TASK_DESCRIPTION_MAX_CHARS: usize = 200;

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
        let mut conn = Connection::open(path)?;
        Self::apply_pragmas(&conn)?;
        Self::init_schema(&mut conn)?;
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

    fn init_schema(conn: &mut Connection) -> Result<()> {
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
            CREATE TABLE IF NOT EXISTS trace_tasks (
                trace_id TEXT PRIMARY KEY,
                task_description TEXT NOT NULL,
                start_time INTEGER NOT NULL
            );
            "#,
        )?;
        request_store::init(conn)?;
        let alter_result = conn.execute(
            "ALTER TABLE spans ADD COLUMN last_updated_at INTEGER NOT NULL DEFAULT 0",
            [],
        );
        if let Err(error) = alter_result {
            if !error.to_string().contains("duplicate column name") {
                return Err(error.into());
            }
        }
        Self::run_migrations(conn)
    }

    /// Run one-time migrations gated by `PRAGMA user_version`: interrupted
    /// migrations are retried on the next open, completed ones never run
    /// again. Steps must be idempotent so retries are safe.
    fn run_migrations(conn: &mut Connection) -> Result<()> {
        let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version < TRACE_TASKS_MIGRATION_VERSION {
            Self::backfill_trace_tasks(conn)?;
            conn.pragma_update(None, "user_version", TRACE_TASKS_MIGRATION_VERSION)?;
        }
        Ok(())
    }

    /// One-time migration for databases that predate `trace_tasks`: backfill
    /// a task description per trace from its earliest usable LLM_CALL span
    /// (via `effective_request` for spans written before `input_preview`
    /// existed). Each trace is backfilled in its own transaction so
    /// interrupted runs resume incrementally and concurrent runs converge;
    /// unreadable spans are skipped with a warning.
    fn backfill_trace_tasks(conn: &mut Connection) -> Result<()> {
        let started_at = std::time::Instant::now();
        let trace_ids: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT trace_id FROM spans
                 WHERE span_type = ?1
                   AND NOT EXISTS (
                       SELECT 1 FROM trace_tasks tt WHERE tt.trace_id = spans.trace_id
                   )",
            )?;
            let rows = stmt.query_map(rusqlite::params![LLM_CALL_SPAN_TYPE], |row| row.get(0))?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        if trace_ids.is_empty() {
            return Ok(());
        }
        tracing::info!(
            trace_count = trace_ids.len(),
            "backfilling trace_tasks for existing traces"
        );
        let mut backfilled_traces = 0usize;
        let mut unlabeled_traces = 0usize;
        let mut unreadable_spans = 0usize;
        for trace_id in trace_ids {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut span_stmt = tx.prepare_cached(
                "SELECT span_id, trace_id, parent_span_id, span_type, start_time,
                        last_updated_at, end_time, extras, created_at
                 FROM spans
                 WHERE trace_id = ?1 AND span_type = ?2
                 ORDER BY start_time ASC, created_at ASC, span_id ASC",
            )?;
            let mut rows = span_stmt.query(rusqlite::params![trace_id, LLM_CALL_SPAN_TYPE])?;
            let mut labeled = false;
            while let Some(row) = rows.next()? {
                let span_id: String = row.get(0)?;
                let span = match Self::span_from_row(&tx, row) {
                    Ok(span) => span,
                    Err(error) => {
                        tracing::warn!(
                            trace_id = %trace_id,
                            span_id = %span_id,
                            %error,
                            "skipping unreadable span while backfilling trace_tasks"
                        );
                        unreadable_spans += 1;
                        continue;
                    }
                };
                if let Some(task) = Self::task_preview_from_span(&span) {
                    Self::upsert_trace_task(&tx, &trace_id, &task, span.start_time)?;
                    labeled = true;
                    break;
                }
            }
            drop(rows);
            drop(span_stmt);
            tx.commit()?;
            if labeled {
                backfilled_traces += 1;
            } else {
                unlabeled_traces += 1;
            }
        }
        tracing::info!(
            backfilled_traces,
            unlabeled_traces,
            unreadable_spans,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            "trace_tasks backfill complete"
        );
        Ok(())
    }

    /// Upsert a task description, keeping the earliest-starting contributor.
    fn upsert_trace_task(
        conn: &Connection,
        trace_id: &str,
        task_description: &str,
        start_time: i64,
    ) -> Result<()> {
        conn.prepare_cached(
            "INSERT INTO trace_tasks (trace_id, task_description, start_time)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(trace_id) DO UPDATE SET
                 task_description = excluded.task_description,
                 start_time = excluded.start_time
             WHERE excluded.start_time < trace_tasks.start_time",
        )?
        .execute(rusqlite::params![trace_id, task_description, start_time])?;
        Ok(())
    }

    /// On the span write path, record a non-empty LLM_CALL `input_preview`
    /// extra as the trace's task description unless an earlier span did.
    fn materialize_trace_task(
        conn: &Connection,
        trace_id: &str,
        span_type: &str,
        extras: &serde_json::Value,
        start_time: i64,
    ) -> Result<()> {
        if span_type != LLM_CALL_SPAN_TYPE {
            return Ok(());
        }
        let Some(preview) = extras
            .get("input_preview")
            .and_then(serde_json::Value::as_str)
            .and_then(Self::normalize_task_preview)
        else {
            return Ok(());
        };
        Self::upsert_trace_task(conn, trace_id, &preview, start_time)
    }

    /// First `text` block of a serialized message, falling back to the
    /// legacy `content` string.
    fn message_text(message: &serde_json::Value) -> Option<&str> {
        message
            .get("blocks")
            .and_then(serde_json::Value::as_array)
            .and_then(|blocks| {
                blocks
                    .iter()
                    .find_map(|block| block.get("text").and_then(serde_json::Value::as_str))
            })
            .or_else(|| message.get("content").and_then(serde_json::Value::as_str))
    }

    /// Task preview from a recorded span: prefer the inline `input_preview`
    /// extra, then the last user message of `effective_request`. Mirrors
    /// llm-client's `user_input_preview`; keep in sync manually.
    fn task_preview_from_span(span: &Span) -> Option<String> {
        if let Some(preview) = span
            .extras
            .get("input_preview")
            .and_then(serde_json::Value::as_str)
            .and_then(Self::normalize_task_preview)
        {
            return Some(preview);
        }
        span.extras
            .get("effective_request")?
            .get("messages")?
            .as_array()?
            .iter()
            .rev()
            .filter(|message| {
                message.get("role").and_then(serde_json::Value::as_str) == Some("user")
            })
            .find_map(Self::message_text)
            .and_then(Self::normalize_task_preview)
    }

    /// Normalize text into a bounded single-line preview: collapse
    /// `\n`/`\r`/`\t` to spaces, drop other control chars, trim, and cap at
    /// `TASK_DESCRIPTION_MAX_CHARS` with an ellipsis. `None` if nothing
    /// visible remains. Mirrors llm-client's `single_line_preview`; keep in
    /// sync manually.
    fn normalize_task_preview(text: &str) -> Option<String> {
        let mut preview = String::new();
        let mut truncated = false;
        for (index, ch) in text.chars().enumerate() {
            if index >= TASK_DESCRIPTION_MAX_CHARS {
                truncated = true;
                break;
            }
            if ch.is_control() {
                match ch {
                    '\n' | '\r' | '\t' => preview.push(' '),
                    _ => {}
                }
                continue;
            }
            preview.push(ch);
        }
        let preview = preview.trim();
        if preview.is_empty() {
            return None;
        }
        let mut preview = preview.to_string();
        if truncated {
            preview.push('…');
        }
        Some(preview)
    }

    fn span_from_row(conn: &Connection, row: &Row) -> Result<Span> {
        let span_type: String = row.get(3)?;
        let extras_str: String = row.get(7)?;
        let mut extras: serde_json::Value = serde_json::from_str(&extras_str)?;
        let span_id: String = row.get(0)?;
        request_store::decode(conn, &span_id, &mut extras)?;

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
            Ok(SqliteStorage::span_from_row(conn, row))
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
            let mut conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let mut extras = span.extras;
            request_store::encode(&tx, &span.span_id, &mut extras)?;
            let extras_str = serde_json::to_string(&extras)?;

            tx.execute(
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
            Self::materialize_trace_task(
                &tx,
                &span.trace_id,
                &span.span_type,
                &extras,
                span.start_time,
            )?;
            tx.commit()?;
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
            let mut conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let (current_extras_str, span_start_time, span_trace_id, span_type): (
                String,
                i64,
                String,
                String,
            ) = tx
                .query_row(
                    "SELECT extras, start_time, trace_id, span_type FROM spans WHERE span_id = ?1",
                    [&span_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                        ))
                    },
                )
                .optional()?
                .ok_or_else(|| MoiraiError::NotFound(format!("Span not found: {}", span_id)))?;

            let mut current_extras: serde_json::Value = serde_json::from_str(&current_extras_str)?;

            if let (serde_json::Value::Object(ref mut current_obj), serde_json::Value::Object(new_obj)) = (&mut current_extras, &extras) {
                for (key, value) in new_obj {
                    current_obj.insert(key.clone(), value.clone());
                }
            }

            request_store::encode(&tx, &span_id, &mut current_extras)?;
            let merged_extras_str = serde_json::to_string(&current_extras)?;

            if let Some(end_time_val) = end_time {
                tx.execute(
                    "UPDATE spans SET extras = ?1, last_updated_at = ?2, end_time = ?3 WHERE span_id = ?4",
                    rusqlite::params![merged_extras_str, last_updated_at, end_time_val, span_id],
                )?;
            } else {
                tx.execute(
                    "UPDATE spans SET extras = ?1, last_updated_at = ?2 WHERE span_id = ?3",
                    rusqlite::params![merged_extras_str, last_updated_at, span_id],
                )?;
            }

            Self::materialize_trace_task(
                &tx,
                &span_trace_id,
                &span_type,
                &extras,
                span_start_time,
            )?;
            tx.commit()?;
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
            let conn = conn.unchecked_transaction()?;

            let mut stmt = conn.prepare(
                "SELECT span_id, trace_id, parent_span_id, span_type, start_time, last_updated_at, end_time, extras, created_at
                 FROM spans WHERE span_id = ?1",
            )?;

            let mut rows = stmt.query(rusqlite::params![span_id])?;

            match rows.next()? {
                Some(row) => Ok(Some(SqliteStorage::span_from_row(&conn, row)?)),
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
            let conn = conn.unchecked_transaction()?;

            let mut stmt = conn.prepare(
                "SELECT span_id, trace_id, parent_span_id, span_type, start_time, last_updated_at, end_time, extras, created_at
                 FROM spans WHERE trace_id = ?1 ORDER BY created_at ASC",
            )?;

            let rows = stmt.query_map(rusqlite::params![trace_id], |row| {
                Ok(SqliteStorage::span_from_row(&conn, row))
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

            let mut stmt = conn.prepare(&format!(
                r#"
                SELECT
                    s.trace_id,
                    COUNT(*) as span_count,
                    MIN(s.start_time) as start_time,
                    (SELECT end_time FROM spans
                     WHERE trace_id = s.trace_id AND span_type = 'END'
                     LIMIT 1) as end_time,
                    (SELECT span_type FROM spans WHERE trace_id = s.trace_id AND parent_span_id IS NULL LIMIT 1) as root_span_type,
                    {TRACE_TASK_DESCRIPTION_SQL}
                FROM spans s
                GROUP BY s.trace_id
                ORDER BY start_time DESC
                LIMIT ?1
                "#,
            ))?;

            let rows = stmt.query_map(rusqlite::params![limit], |row| {
                Ok(TraceSummary {
                    trace_id: row.get(0)?,
                    span_count: row.get(1)?,
                    start_time: row.get(2)?,
                    end_time: row.get(3)?,
                    root_span_type: row.get(4)?,
                    task_description: row.get(5)?,
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

            let mut stmt = conn.prepare(&format!(
                r#"
                SELECT
                    s.trace_id,
                    COUNT(*) as span_count,
                    MIN(s.start_time) as start_time,
                    MAX(s.end_time) as end_time,
                    (SELECT span_type FROM spans WHERE trace_id = s.trace_id AND parent_span_id IS NULL LIMIT 1) as root_span_type,
                    {TRACE_TASK_DESCRIPTION_SQL}
                FROM spans s
                WHERE s.trace_id NOT IN (
                    SELECT DISTINCT trace_id FROM spans WHERE span_type = 'END'
                )
                GROUP BY s.trace_id
                ORDER BY start_time DESC
                LIMIT ?1
                "#,
            ))?;

            let rows = stmt.query_map(rusqlite::params![limit], |row| {
                Ok(TraceSummary {
                    trace_id: row.get(0)?,
                    span_count: row.get(1)?,
                    start_time: row.get(2)?,
                    end_time: row.get(3)?,
                    root_span_type: row.get(4)?,
                    task_description: row.get(5)?,
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
            let conn = conn.unchecked_transaction()?;

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
            let mut conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let deleted = tx.execute(
                "DELETE FROM spans WHERE created_at < ?1",
                rusqlite::params![cutoff_timestamp_ms],
            )?;

            tx.execute(
                "DELETE FROM trace_tasks WHERE NOT EXISTS (
                    SELECT 1 FROM spans s WHERE s.trace_id = trace_tasks.trace_id
                )",
                [],
            )?;
            request_store::collect_unused(&tx)?;
            tx.commit()?;
            Ok(deleted)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }

    pub async fn delete_trace(&self, trace_id: &str) -> Result<usize> {
        let conn = self.conn.clone();
        let trace_id = trace_id.to_string();

        tokio::task::spawn_blocking(move || {
            let mut conn = conn
                .lock()
                .map_err(|e| MoiraiError::Storage(e.to_string()))?;

            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let deleted = tx.execute(
                "DELETE FROM spans WHERE trace_id = ?1",
                rusqlite::params![trace_id],
            )?;

            tx.execute(
                "DELETE FROM trace_tasks WHERE trace_id = ?1",
                rusqlite::params![trace_id],
            )?;
            request_store::collect_unused(&tx)?;
            tx.commit()?;
            Ok(deleted)
        })
        .await
        .map_err(|e| MoiraiError::Storage(e.to_string()))?
    }
}

#[cfg(test)]
#[path = "../../../../../../../../tests/unit/trace/moirai/storage/span/implementations/sqlite_test.rs"]
mod tests;
