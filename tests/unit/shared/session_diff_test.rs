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

#[test]
fn file_write_events_are_per_call_while_session_totals_remain_net() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let workspace = temp.path().join("workspace");
    fs::write(workspace.join("a.txt"), "seed\n").unwrap();

    let writes = [
        ("a.txt", "seed\none\n", (1, 0), Some((1, 0)), false),
        ("a.txt", "seed\none\ntwo\n", (1, 0), Some((2, 0)), false),
        (
            "a.txt",
            "seed\none\ntwo\nthree\n",
            (1, 0),
            Some((3, 0)),
            true,
        ),
        ("a.txt", "seed\n", (0, 3), None, true),
        ("new.txt", "new\n", (1, 0), Some((1, 0)), true),
        ("empty.txt", "", (0, 0), None, true),
    ];
    for (index, (path, content, expected, session, next_turn)) in writes.into_iter().enumerate() {
        if next_turn {
            tracker.clear_per_turn_state();
        }
        let call_id = format!("write-{index}");
        let args = serde_json::json!({"file_path": path, "content": content}).to_string();
        tracker.on_tool_running(&call_id, "file_write", &args);
        fs::write(workspace.join(path), content).unwrap();
        let delta = tracker
            .on_tool_completed(&call_id, "file_write", &args, None)
            .expect("successful write emits a delta, including empty creation");
        assert_eq!(delta.file_path, path);
        assert_eq!((delta.additions, delta.deletions), expected, "{call_id}");
        assert_eq!(fs::read_to_string(workspace.join(path)).unwrap(), content);
        assert_eq!(
            tracker
                .session_file_changes()
                .get(path)
                .map(|s| (s.additions, s.deletions)),
            session,
            "session net diff for {call_id}"
        );
        let totals = tracker.session_file_changes().clone();
        assert!(tracker
            .on_tool_completed(&call_id, "file_write", &args, None)
            .is_none());
        assert_eq!(tracker.session_file_changes(), &totals);
    }
}

#[test]
fn file_edit_events_use_each_calls_content_across_turns() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let file = temp.path().join("workspace/a.txt");
    fs::write(&file, "seed\n").unwrap();
    for (index, (before, after, expected)) in [
        ("seed\n", "seed\none\n", (1, 0)),
        ("seed\none\n", "seed\none\ntwo\n", (1, 0)),
        ("seed\none\ntwo\n", "seed\n", (0, 2)),
    ]
    .into_iter()
    .enumerate()
    {
        tracker.clear_per_turn_state();
        let call_id = format!("edit-{index}");
        let args =
            serde_json::json!({"file_path": "a.txt", "old_string": before, "new_string": after})
                .to_string();
        tracker.on_tool_running(&call_id, "file_edit", &args);
        fs::write(&file, after).unwrap();
        let delta = tracker
            .on_tool_completed(&call_id, "file_edit", &args, None)
            .unwrap();
        assert_eq!((delta.additions, delta.deletions), expected);
    }
    assert!(tracker.session_file_changes().is_empty());
}

#[test]
fn injected_baselines_keep_per_call_content_and_first_session_content() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let workspace = temp.path().join("workspace");
    let file = workspace.join("a.txt");
    fs::write(&file, "seed\n").unwrap();
    for (index, content) in ["seed\none\n", "seed\none\ntwo\n"].into_iter().enumerate() {
        let args = serde_json::json!({"file_path": "a.txt", "content": content}).to_string();
        let call_id = format!("injected-{index}");
        let payload = capture_file_baseline_payload(&workspace, "file_write", &args).unwrap();
        fs::write(&file, content).unwrap();
        tracker.inject_session_file_baseline(&call_id, &payload);
        tracker.on_tool_running(&call_id, "file_write", &args);
        let delta = tracker
            .on_tool_completed(&call_id, "file_write", &args, None)
            .unwrap();
        assert_eq!((delta.additions, delta.deletions), (1, 0));
        assert_eq!(
            tracker.session_file_changes()["a.txt"].additions,
            index as u32 + 1
        );
    }
}

#[test]
fn unreadable_pre_execution_content_keeps_args_estimation() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let file = temp.path().join("workspace/a.txt");
    fs::write(&file, [0xff]).unwrap();
    let args = r#"{"file_path":"a.txt","content":"one\ntwo\n"}"#;
    tracker.on_tool_running("write", "file_write", args);
    fs::write(&file, "one\ntwo\n").unwrap();
    let delta = tracker
        .on_tool_completed("write", "file_write", args, None)
        .unwrap();
    assert_eq!((delta.additions, delta.deletions), (2, 0));
}

#[test]
fn unreadable_post_execution_content_keeps_args_estimation() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let file = temp.path().join("workspace/a.txt");
    fs::write(&file, "seed\n").unwrap();
    let args = r#"{"file_path":"a.txt","old_string":"seed","new_string":"one\ntwo"}"#;
    tracker.on_tool_running("edit", "file_edit", args);
    fs::write(&file, [0xff]).unwrap();
    let delta = tracker
        .on_tool_completed("edit", "file_edit", args, None)
        .unwrap();
    assert_eq!((delta.additions, delta.deletions), (2, 1));
}

#[test]
fn empty_file_creation_emits_a_zero_delta_once() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let args = r#"{"file_path":"empty.txt","content":""}"#;
    tracker.on_tool_running("empty", "file_write", args);
    fs::write(temp.path().join("workspace/empty.txt"), "").unwrap();
    let delta = tracker
        .on_tool_completed("empty", "file_write", args, None)
        .expect("empty creation emits a delta");
    assert_eq!(delta.file_path, "empty.txt");
    assert_eq!((delta.additions, delta.deletions), (0, 0));
    assert!(tracker
        .on_tool_completed("empty", "file_write", args, None)
        .is_none());
    assert!(tracker.session_file_changes().is_empty());
}

#[test]
fn empty_write_without_reliable_baseline_does_not_invent_creation() {
    let (_temp, mut tracker) = tracker_in_temp_workspace();
    let args = r#"{"file_path":"empty.txt","content":""}"#;
    assert!(tracker
        .on_tool_completed("write", "file_write", args, None)
        .is_none());
    assert!(tracker.session_file_changes().is_empty());
}

#[test]
fn failed_and_other_tools_do_not_emit_file_changes() {
    let (temp, mut tracker) = tracker_in_temp_workspace();
    let file = temp.path().join("workspace/a.txt");
    fs::write(&file, "seed\n").unwrap();
    let args = r#"{"file_path":"a.txt","content":"seed\none\n"}"#;
    tracker.on_tool_running("write", "file_write", args);
    assert!(tracker.on_tool_failed("write", None).is_none());
    assert!(tracker.tool_file_baselines.is_empty());
    tracker.on_tool_running("shell", "bash", r#"{"command":"rm a.txt"}"#);
    fs::remove_file(&file).unwrap();
    assert!(tracker
        .on_tool_completed("shell", "bash", r#"{"command":"rm a.txt"}"#, None)
        .is_none());
    assert!(tracker.session_file_changes().is_empty());
}
