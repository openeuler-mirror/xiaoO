use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::unbounded_channel;
use xiaoo_api::chat::AgentId;
use xiaoo_api::events::{LoopEventSink, ToolEventSink};
use xiaoo_api::events::{ToolLifecycleEvent, ToolResultEvent};

use super::{ChannelLoopEventSink, ChannelToolEventSink, SessionTurnUpdate};

#[test]
fn loop_tool_result_forwards_args_preview() {
    let (tx, mut rx) = unbounded_channel();
    let sink = ChannelLoopEventSink::new(tx, Arc::new(Mutex::new(None)));

    sink.on_tool_result(
        &AgentId("root".to_string()),
        &ToolResultEvent {
            call_id: "call-1".to_string(),
            tool_name: "spawn_subagent".to_string(),
            output_preview: "{\"agent_id\":\"child\"}".to_string(),
            is_error: false,
            args_preview: "{\n  \"task_goal\": \"run\"\n}".to_string(),
        },
    );

    let SessionTurnUpdate::Tool { update, .. } = rx.try_recv().expect("tool update expected")
    else {
        panic!("expected tool update");
    };
    assert_eq!(update.args_preview, "{\n  \"task_goal\": \"run\"\n}");
    assert!(update.file_change.is_none());
}

#[test]
fn lifecycle_tool_event_forwards_args_preview() {
    let (tx, mut rx) = unbounded_channel();
    let sink = ChannelToolEventSink::new(tx, std::path::PathBuf::from("."));

    sink.emit(ToolLifecycleEvent::Running {
        call_id: "call-2".to_string(),
        tool_name: "join_subagent".to_string(),
        args_preview: "{\n  \"target_agent_id\": \"child\"\n}".to_string(),
    });

    let SessionTurnUpdate::Tool { update, .. } = rx.try_recv().expect("tool update expected")
    else {
        panic!("expected tool update");
    };
    assert_eq!(
        update.args_preview,
        "{\n  \"target_agent_id\": \"child\"\n}"
    );
    assert!(update.file_change.is_none());
}

#[test]
fn scoped_lifecycle_tool_event_forwards_agent_id() {
    let (tx, mut rx) = unbounded_channel();
    let sink = ChannelToolEventSink::new(tx, std::path::PathBuf::from("."));

    sink.emit(
        ToolLifecycleEvent::Running {
            call_id: "call-child".to_string(),
            tool_name: "bash".to_string(),
            args_preview: "{}".to_string(),
        }
        .scoped(AgentId("child-agent".to_string())),
    );

    let SessionTurnUpdate::Tool {
        agent_id, update, ..
    } = rx.try_recv().expect("tool update expected")
    else {
        panic!("expected tool update");
    };
    assert_eq!(agent_id.0, "child-agent");
    assert_eq!(update.call_id, "call-child");
}

/// Regression (local-mode race): for a file-targeting tool the sink
/// must enqueue the pre-execution baseline BEFORE the `Tool` update
/// so the TUI freezes the true "before" content even though it
/// drains the channel after the tool already finished.
#[test]
fn file_tool_running_emits_baseline_before_tool_update() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().join("workspace");
    std::fs::create_dir_all(workspace.join("src")).unwrap();
    std::fs::write(workspace.join("src/main.rs"), "before\n").unwrap();

    let (tx, mut rx) = unbounded_channel();
    let sink = ChannelToolEventSink::new(tx, workspace.clone());

    sink.emit(ToolLifecycleEvent::Running {
        call_id: "call-x".to_string(),
        tool_name: "file_edit".to_string(),
        args_preview: r#"{"file_path":"src/main.rs","old_string":"before","new_string":"after"}"#
            .to_string(),
    });

    let first = rx.try_recv().expect("first message expected");
    let SessionTurnUpdate::ToolBaseline { call_id, payload } = first else {
        panic!("expected ToolBaseline ahead of the Tool update");
    };
    assert_eq!(call_id, "call-x");
    assert_eq!(payload.file_path, "src/main.rs");
    assert_eq!(payload.content.as_deref(), Some("before\n"));

    let SessionTurnUpdate::Tool { update, .. } = rx.try_recv().expect("tool update expected")
    else {
        panic!("expected tool update");
    };
    assert_eq!(update.call_id, "call-x");
}
