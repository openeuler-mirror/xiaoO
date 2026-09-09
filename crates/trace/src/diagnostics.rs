use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TraceDiagnosticSummary {
    pub trace_id: String,
    pub span_count: u64,
    pub start_time_ms: i64,
    pub end_time_ms: Option<i64>,
    pub root_span_type: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct HookExecutionDiagnosticSummary {
    pub trace_id: String,
    pub hooker_id: String,
    pub hook_point: String,
    pub category: String,
    pub outcome: String,
    pub start_time_ms: i64,
    pub end_time_ms: Option<i64>,
}

pub fn default_trace_db_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".xiaoo/data/trace.db")
}

/// Read recent trace metadata without creating or migrating the database.
pub fn inspect_trace_database(
    path: &Path,
    limit: usize,
) -> Result<Vec<TraceDiagnosticSummary>, String> {
    let path = path
        .to_str()
        .ok_or_else(|| "trace database path is not valid UTF-8".to_string())?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "failed to open trace database for reading".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT s.trace_id, COUNT(*) AS span_count, MIN(s.start_time) AS start_time, \
             CASE WHEN SUM(CASE WHEN s.end_time IS NULL THEN 1 ELSE 0 END) > 0 \
                  THEN NULL ELSE MAX(s.end_time) END AS end_time, \
             COALESCE((SELECT root.span_type FROM spans root \
                       WHERE root.trace_id = s.trace_id AND root.parent_span_id IS NULL \
                       ORDER BY root.start_time ASC LIMIT 1), 'unknown') AS root_span_type \
             FROM spans s GROUP BY s.trace_id ORDER BY start_time DESC LIMIT ?1",
        )
        .map_err(|_| "trace database schema is unavailable".to_string())?;
    let rows = statement
        .query_map([limit.min(100) as i64], |row| {
            Ok(TraceDiagnosticSummary {
                trace_id: row.get(0)?,
                span_count: row.get::<_, i64>(1)?.max(0) as u64,
                start_time_ms: row.get(2)?,
                end_time_ms: row.get(3)?,
                root_span_type: row.get(4)?,
            })
        })
        .map_err(|_| "failed to query trace database".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "failed to decode trace metadata".to_string())
}

/// Read recent Hook execution metadata without returning invocation payloads or arbitrary extras.
pub fn inspect_recent_hook_executions(
    path: &Path,
    limit: usize,
) -> Result<Vec<HookExecutionDiagnosticSummary>, String> {
    let path = path
        .to_str()
        .ok_or_else(|| "trace database path is not valid UTF-8".to_string())?;
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| "failed to open trace database for reading".to_string())?;
    let mut statement = connection
        .prepare(
            "SELECT trace_id, \
             json_extract(extras, '$.hooker_id'), \
             json_extract(extras, '$.hook_point'), \
             json_extract(extras, '$.hook_kind'), \
             json_extract(extras, '$.outcome'), start_time, end_time \
             FROM spans WHERE span_type = 'HOOK' \
             AND json_type(extras, '$.hooker_id') = 'text' \
             AND json_type(extras, '$.hook_point') = 'text' \
             ORDER BY start_time DESC LIMIT ?1",
        )
        .map_err(|_| "trace database schema is unavailable".to_string())?;
    let rows = statement
        .query_map([limit.min(100) as i64], |row| {
            Ok(HookExecutionDiagnosticSummary {
                trace_id: row.get(0)?,
                hooker_id: row.get(1)?,
                hook_point: row.get(2)?,
                category: row
                    .get::<_, Option<String>>(3)?
                    .unwrap_or_else(|| "unknown".to_string()),
                outcome: row
                    .get::<_, Option<String>>(4)?
                    .unwrap_or_else(|| "running".to_string())
                    .to_ascii_lowercase(),
                start_time_ms: row.get(5)?,
                end_time_ms: row.get(6)?,
            })
        })
        .map_err(|_| "failed to query trace database".to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|_| "failed to decode hook execution metadata".to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        inspect_recent_hook_executions, inspect_trace_database, HookExecutionDiagnosticSummary,
        TraceDiagnosticSummary,
    };
    use rusqlite::Connection;
    use tempfile::TempDir;

    #[test]
    fn reads_recent_metadata_without_returning_span_extras() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("trace.db");
        let connection = Connection::open(&path).expect("database");
        connection
            .execute_batch(
                "CREATE TABLE spans (span_id TEXT PRIMARY KEY, trace_id TEXT NOT NULL, parent_span_id TEXT, span_type TEXT NOT NULL, start_time INTEGER NOT NULL, last_updated_at INTEGER NOT NULL, end_time INTEGER, extras TEXT NOT NULL, created_at INTEGER NOT NULL);\
                 INSERT INTO spans VALUES ('root-1', 'trace-1', NULL, 'agent', 100, 200, 200, '{\"secret\":\"never-return\"}', 100);\
                 INSERT INTO spans VALUES ('child-1', 'trace-1', 'root-1', 'tool', 120, 180, 180, '{}', 120);",
            )
            .expect("fixture");
        drop(connection);

        assert_eq!(
            inspect_trace_database(&path, 20).expect("diagnostics"),
            vec![TraceDiagnosticSummary {
                trace_id: "trace-1".to_string(),
                span_count: 2,
                start_time_ms: 100,
                end_time_ms: Some(200),
                root_span_type: "agent".to_string(),
            }]
        );
    }

    #[test]
    fn does_not_create_a_missing_database() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("missing.db");
        assert!(inspect_trace_database(&path, 20).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn reads_only_allowlisted_hook_execution_metadata() {
        let temp = TempDir::new().expect("tempdir");
        let path = temp.path().join("trace.db");
        let connection = Connection::open(&path).expect("database");
        connection
            .execute_batch(
                "CREATE TABLE spans (span_id TEXT PRIMARY KEY, trace_id TEXT NOT NULL, parent_span_id TEXT, span_type TEXT NOT NULL, start_time INTEGER NOT NULL, last_updated_at INTEGER NOT NULL, end_time INTEGER, extras TEXT NOT NULL, created_at INTEGER NOT NULL);\
                 INSERT INTO spans VALUES ('hook-1', 'trace-1', 'root-1', 'HOOK', 120, 180, 180, '{\"hooker_id\":\"audit\",\"hook_point\":\"agent.Tool.bash.pre\",\"hook_kind\":\"tool_pre\",\"outcome\":\"Ok\",\"secret\":\"never-return\"}', 120);\
                 INSERT INTO spans VALUES ('tool-1', 'trace-1', 'root-1', 'TOOL_CALL', 100, 200, 200, '{}', 100);",
            )
            .expect("fixture");
        drop(connection);

        assert_eq!(
            inspect_recent_hook_executions(&path, 20).expect("hook diagnostics"),
            vec![HookExecutionDiagnosticSummary {
                trace_id: "trace-1".to_string(),
                hooker_id: "audit".to_string(),
                hook_point: "agent.Tool.bash.pre".to_string(),
                category: "tool_pre".to_string(),
                outcome: "ok".to_string(),
                start_time_ms: 120,
                end_time_ms: Some(180),
            }]
        );
    }
}
