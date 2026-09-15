use super::*;

fn tracker_in_temp_workspace() -> (tempfile::TempDir, SessionDiffTracker) {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let tracker = SessionDiffTracker::new(workspace);
    (temp, tracker)
}

#[test]
fn running_then_completed_for_new_file_counts_addition() {
    let (_temp, mut tracker) = tracker_in_temp_workspace();
    tracker.on_tool_running(
        "call-1",
        "file_write",
        r#"{"file_path":"aaa","content":"aaabbb"}"#,
    );
    let delta = tracker
        .on_tool_completed(
            "call-1",
            "file_write",
            r#"{"file_path":"aaa","content":"aaabbb"}"#,
            None,
        )
        .expect("delta should be computed");
    assert_eq!(delta.file_path, "aaa");
    assert_eq!(delta.additions, 1);
    assert_eq!(delta.deletions, 0);

    let entries = tracker.sorted_session_file_changes();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].additions, 1);
    assert_eq!(entries[0].deletions, 0);
}

#[test]
fn file_edit_existing_file_counts_addition_and_deletion() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let file = temp.path().join("workspace/README.md");
    fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").expect("baseline");

    tracker.on_tool_running(
        "call-1",
        "file_edit",
        r#"{"file_path":"README.md","old_string":"three","new_string":"THREE"}"#,
    );
    fs::write(&file, "one\ntwo\nTHREE\nfour\nfive\n").expect("modified");
    let delta = tracker
        .on_tool_completed(
            "call-1",
            "file_edit",
            r#"{"file_path":"README.md","old_string":"three","new_string":"THREE"}"#,
            None,
        )
        .expect("delta should be computed");
    assert_eq!(delta.additions, 1);
    assert_eq!(delta.deletions, 1);
}

#[test]
fn apply_remote_delta_directly_records_change() {
    let (_temp, mut tracker) = tracker_in_temp_workspace();
    tracker.apply_remote_delta(
        "remote-call-1",
        FileChangeDelta {
            file_path: "src/lib.rs".to_string(),
            additions: 5,
            deletions: 2,
        },
    );
    let entries = tracker.sorted_session_file_changes();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_path, "src/lib.rs");
    assert_eq!(entries[0].additions, 5);
    assert_eq!(entries[0].deletions, 2);
}

#[test]
fn clear_per_turn_state_drops_per_call_maps_but_preserves_file_totals() {
    // Regression: in remote mode the TUI invokes `clear_per_turn_state`
    // from `finish_stream_done` (no `LoopEnd` SSE event is forwarded by
    // the daemon). The contract: per-call `tool_file_changes` and
    // `tool_file_baselines` must be emptied so they do not grow
    // unboundedly across turns, while long-lived per-file totals
    // (`session_file_changes`) must survive.
    let (_temp, mut tracker) = tracker_in_temp_workspace();
    tracker.apply_remote_delta(
        "call-1",
        FileChangeDelta {
            file_path: "src/a.rs".to_string(),
            additions: 3,
            deletions: 1,
        },
    );
    tracker.apply_remote_delta(
        "call-2",
        FileChangeDelta {
            file_path: "src/b.rs".to_string(),
            additions: 2,
            deletions: 0,
        },
    );
    // Populate a baseline entry as well (path is irrelevant for the
    // contract; we only need a non-empty `tool_file_baselines` map).
    tracker.capture_tool_file_baseline(
        "call-3",
        "file_write",
        r#"{"file_path":"src/c.rs","content":"new"}"#,
    );

    assert_eq!(tracker.tool_file_changes.len(), 2);
    assert_eq!(tracker.tool_file_baselines.len(), 1);
    assert_eq!(tracker.session_file_changes.len(), 2);

    tracker.clear_per_turn_state();

    assert!(tracker.tool_file_changes.is_empty());
    assert!(tracker.tool_file_baselines.is_empty());
    // Per-file totals must survive so the diff panel keeps showing the
    // session's cumulative changes after the turn ends.
    assert_eq!(tracker.session_file_changes.len(), 2);
    let entries = tracker.sorted_session_file_changes();
    assert_eq!(entries[0].file_path, "src/a.rs");
    assert_eq!(entries[0].additions, 3);
    assert_eq!(entries[0].deletions, 1);
    assert_eq!(entries[1].file_path, "src/b.rs");
    assert_eq!(entries[1].additions, 2);
    assert_eq!(entries[1].deletions, 0);
}

/// Local-TUI race regression: the Running lifecycle event is queued
/// and drained by the UI loop only after the tool already finished.
/// The sink captures the true pre-execution content on the executor
/// thread and injects it via `inject_session_file_baseline`; the
/// later delayed `capture_tool_file_baseline` (which would read the
/// already-modified file) must NOT win.
#[test]
fn injected_pre_execution_baseline_wins_over_delayed_capture() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let file = temp.path().join("workspace/src/main.rs");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "fn main() {\n    println!(\"hi\");\n}\n").unwrap();

    // 1) Sink-side synchronous capture (BEFORE the tool runs).
    let payload = capture_file_baseline_payload(
        temp.path().join("workspace").as_path(),
        "file_edit",
        r#"{"file_path":"src/main.rs","old_string":"hi","new_string":"hello"}"#,
    )
    .expect("payload");
    assert_eq!(
        payload.content.as_deref(),
        Some("fn main() {\n    println!(\"hi\");\n}\n")
    );

    // 2) Tool executes and modifies the file.
    fs::write(&file, "fn main() {\n    println!(\"hello\");\n}\n").unwrap();

    // 3) UI loop drains the injected baseline first...
    tracker.inject_session_file_baseline("call-1", &payload);
    // ...then the delayed Running-driven capture, which now reads the
    // POST-execution content and must be a no-op for this file.
    tracker.on_tool_running(
        "call-1",
        "file_edit",
        r#"{"file_path":"src/main.rs","old_string":"hi","new_string":"hello"}"#,
    );
    // 4) Completion reconciles against the injected pre-execution baseline.
    let delta = tracker
        .on_tool_completed(
            "call-1",
            "file_edit",
            r#"{"file_path":"src/main.rs","old_string":"hi","new_string":"hello"}"#,
            None,
        )
        .expect("delta");
    assert_eq!(delta.additions, 1);
    assert_eq!(delta.deletions, 1);
}
