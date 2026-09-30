use super::SqliteStorage;
use crate::{Span, SpanStorage};

fn request_counts(storage: &SqliteStorage) -> (i64, i64, i64) {
    let conn = storage.conn.lock().unwrap();
    conn.query_row(
        "SELECT (SELECT COUNT(*) FROM request_blobs_v1),
                (SELECT COUNT(*) FROM request_prefixes_v1),
                (SELECT COUNT(*) FROM span_requests_v1)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

#[tokio::test]
async fn growing_requests_store_linear_prefixes_and_round_trip_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("requests.db");
    let storage = SqliteStorage::new(path.to_str().unwrap()).unwrap();
    let mut messages = Vec::new();
    let mut expected = Vec::new();
    for i in 0..100 {
        messages.push(serde_json::json!({"role": "user", "content": format!("message {i}")}));
        let extras = serde_json::json!({
            "phase": "started",
            "effective_request": {"messages": messages, "tools": [{"name": "shell"}], "temperature": 0.5}
        });
        storage
            .insert_span(&span(
                &format!("s{i}"),
                "trace",
                None,
                "LLM_CALL",
                i,
                None,
                extras.clone(),
            ))
            .await
            .unwrap();
        expected.push(extras);
    }
    // 100 unique messages + one tools blob + one metadata blob. No per-turn
    // message ID arrays: each request stores exactly one head reference.
    assert_eq!(request_counts(&storage), (102, 100, 100));
    {
        let conn = storage.conn.lock().unwrap();
        let inline: i64 = conn.query_row(
            "SELECT COUNT(*) FROM spans WHERE json_type(extras, '$.effective_request') IS NOT NULL",
            [], |row| row.get(0),
        ).unwrap();
        assert_eq!(inline, 0);
    }
    drop(storage);
    let storage = SqliteStorage::new(path.to_str().unwrap()).unwrap();
    let spans = storage.get_trace_spans("trace").await.unwrap();
    assert_eq!(spans.len(), expected.len());
    for (span, extras) in spans.iter().zip(&expected) {
        assert_eq!(&span.extras, extras);
    }
    assert_eq!(
        storage.get_span("s99").await.unwrap().unwrap().extras,
        expected[99]
    );
    assert_eq!(
        storage
            .get_trace_spans_with_related_segments("trace")
            .await
            .unwrap()
            .len(),
        100
    );
}

#[tokio::test]
async fn request_rewrites_branch_and_status_updates_keep_references() {
    let storage = SqliteStorage::new(":memory:").unwrap();
    for (id, messages) in [
        ("a", vec!["a", "b", "c"]),
        ("b", vec!["a", "changed", "c"]),
        ("c", vec!["a", "b", "c", "d"]),
    ] {
        storage
            .insert_span(&span(
                id,
                id,
                None,
                "LLM_CALL",
                1,
                None,
                serde_json::json!({"effective_request": {"messages": messages}}),
            ))
            .await
            .unwrap();
    }
    assert_eq!(request_counts(&storage), (6, 6, 3));
    storage
        .update_span_extras("b", serde_json::json!({"phase": "done"}), 2, Some(2))
        .await
        .unwrap();
    assert_eq!(request_counts(&storage), (6, 6, 3));
    let read = storage.get_span("b").await.unwrap().unwrap();
    assert_eq!(
        read.extras["effective_request"]["messages"],
        serde_json::json!(["a", "changed", "c"])
    );
    assert_eq!(read.end_time, Some(2));
    storage
        .update_span_extras(
            "b",
            serde_json::json!({"effective_request": {"messages": [], "tools": null}}),
            3,
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        storage.get_span("b").await.unwrap().unwrap().extras["effective_request"],
        serde_json::json!({"messages": [], "tools": null})
    );
    storage
        .update_span_extras("b", serde_json::json!({"effective_request": null}), 4, None)
        .await
        .unwrap();
    assert_eq!(
        storage.get_span("b").await.unwrap().unwrap().extras["effective_request"],
        serde_json::Value::Null
    );
    assert_eq!(request_counts(&storage).2, 2);
}

#[tokio::test]
async fn legacy_requests_read_and_migrate_on_update_without_changing_json() {
    let storage = SqliteStorage::new(":memory:").unwrap();
    let extras = serde_json::json!({"effective_request": {"messages": [{"z": 1, "a": {"y": 2, "b": 3}}], "tools": []}});
    storage
        .insert_span(&span(
            "old",
            "trace",
            None,
            "LLM_CALL",
            1,
            None,
            serde_json::json!({}),
        ))
        .await
        .unwrap();
    storage
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE spans SET extras = ?1 WHERE span_id='old'",
            [extras.to_string()],
        )
        .unwrap();
    assert_eq!(
        storage.get_span("old").await.unwrap().unwrap().extras,
        extras
    );
    assert_eq!(request_counts(&storage), (0, 0, 0));
    storage
        .update_span_extras("old", serde_json::json!({"phase": "done"}), 2, None)
        .await
        .unwrap();
    assert_eq!(
        storage.get_span("old").await.unwrap().unwrap().extras["effective_request"],
        extras["effective_request"]
    );
    let reordered = serde_json::from_str(
        r#"{"effective_request":{"tools":[],"messages":[{"a":{"b":3,"y":2},"z":1}]}}"#,
    )
    .unwrap();
    storage
        .insert_span(&span("new", "trace", None, "LLM_CALL", 2, None, reordered))
        .await
        .unwrap();
    assert_eq!(request_counts(&storage), (3, 1, 2));
}

#[tokio::test]
async fn failed_insert_rolls_back_request_objects_and_reference_changes() {
    let storage = SqliteStorage::new(":memory:").unwrap();
    let original = span(
        "a",
        "trace",
        None,
        "LLM_CALL",
        1,
        None,
        serde_json::json!({"effective_request": {"messages": ["original"]}}),
    );
    storage.insert_span(&original).await.unwrap();
    let counts = request_counts(&storage);
    let replacement = span(
        "a",
        "trace",
        None,
        "LLM_CALL",
        1,
        None,
        serde_json::json!({"effective_request": {"messages": ["different"], "tools": [1]}}),
    );
    assert!(storage.insert_span(&replacement).await.is_err());
    assert_eq!(request_counts(&storage), counts);
    assert_eq!(
        storage.get_span("a").await.unwrap().unwrap().extras,
        original.extras
    );
    assert!(storage
        .update_span_extras("missing", replacement.extras, 2, None)
        .await
        .is_err());
    assert_eq!(request_counts(&storage), counts);
}

#[tokio::test]
async fn deletion_keeps_shared_prefixes_and_collects_unreferenced_payloads() {
    let storage = SqliteStorage::new(":memory:").unwrap();
    for (id, time, messages) in [("old", 1, vec!["a", "b"]), ("new", 2, vec!["a", "b", "c"])] {
        storage
            .insert_span(&span(
                id,
                id,
                None,
                "LLM_CALL",
                time,
                None,
                serde_json::json!({"effective_request": {"messages": messages}}),
            ))
            .await
            .unwrap();
    }
    assert_eq!(storage.delete_old_spans(2).await.unwrap(), 1);
    assert_eq!(request_counts(&storage), (4, 3, 1));
    assert_eq!(
        storage.get_span("new").await.unwrap().unwrap().extras["effective_request"]["messages"],
        serde_json::json!(["a", "b", "c"])
    );
    storage
        .update_span_extras(
            "new",
            serde_json::json!({"effective_request": {"messages": ["replacement"]}}),
            3,
            None,
        )
        .await
        .unwrap();
    assert_eq!(storage.delete_trace("new").await.unwrap(), 1);
    assert_eq!(request_counts(&storage), (0, 0, 0));
}

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
