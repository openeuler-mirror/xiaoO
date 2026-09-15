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
