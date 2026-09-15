use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use xiaoo_api::chat::AgentId;

use crate::app_state::AppState;
use crate::chat::{
    Message, MessageRole, TodoDisplayStatus, ToolExecutionStatus, ToolExecutionUpdate,
};
use crate::gateway::MemoryAutomationHealth;
use crate::session_gateway::SessionTurnUpdate;
use crate::status_panel::MemoryStatus;

use super::{GatewayRuntime, PendingStreamDone};

fn test_state() -> AppState {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("test app state should initialize");
    state.chat_state.messages.clear();
    state
}

fn sample_tool_update(call_id: &str) -> ToolExecutionUpdate {
    ToolExecutionUpdate {
        call_id: call_id.to_string(),
        tool: "shell".to_string(),
        summary: "running".to_string(),
        args_preview: String::new(),
        command_preview: None,
        command: None,
        detail: String::new(),
        status: ToolExecutionStatus::Running,
        exit_code: None,
        duration_ms: None,
        file_change: None,
    }
}

#[test]
fn todo_write_completed_update_updates_right_panel_plan() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "todo-1".to_string(),
            tool: "todo_write".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "todos": [
                    { "content": "Inspect current implementation", "status": "completed" },
                    { "content": "Add todo_write tool", "status": "in_progress" }
                ]
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        },
        None,
    );

    assert!(state.chat_state.messages.is_empty());
    let todo = state
        .plan_state
        .as_ref()
        .expect("todo snapshot should update right panel plan");
    assert_eq!(todo.items.len(), 2);
    assert_eq!(todo.items[0].0, TodoDisplayStatus::Completed);
    assert_eq!(todo.items[1].0, TodoDisplayStatus::InProgress);
}

/// Regression: `apply_todo_snapshot` must invalidate the root layout cache
/// (`cached_area`) on a Some<->None `plan_state` transition, because
/// sidebar visibility depends on `plan_state.is_some()` (see `App::ui`).
/// Content-only updates (still `Some`) must NOT invalidate, so the layout
/// isn't recomputed on every plan-item tick.
#[test]
fn apply_todo_snapshot_invalidates_layout_cache_on_presence_transition() {
    use ratatui::layout::Rect;
    use xiaoo_shared::plan::{
        TodoDisplayStatus as SharedTodoStatus, TodoSnapshotItem, TodoSnapshotUpdate,
    };

    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    // Simulate a prior layout pass that cached the terminal area.
    let cached_rect = Rect::new(0, 0, 64, 24);
    state.render_state.cached_area = Some(cached_rect);

    // plan_state starts None; empty update -> no transition -> cache stays.
    runtime.apply_todo_snapshot(
        &mut state,
        TodoSnapshotUpdate {
            title: String::new(),
            items: vec![],
        },
    );
    assert!(state.plan_state.is_none());
    assert_eq!(
        state.render_state.cached_area,
        Some(cached_rect),
        "None->None transition must not invalidate layout cache"
    );

    // None -> Some transition: cache must be invalidated.
    runtime.apply_todo_snapshot(
        &mut state,
        TodoSnapshotUpdate {
            title: "Plan".to_string(),
            items: vec![TodoSnapshotItem {
                status: SharedTodoStatus::InProgress,
                content: "step 1".to_string(),
            }],
        },
    );
    assert!(state.plan_state.is_some());
    assert!(
        state.render_state.cached_area.is_none(),
        "None->Some transition must invalidate layout cache (sidebar visibility changed)"
    );

    // Re-cache, then Some -> Some (content only): cache must stay.
    state.render_state.cached_area = Some(cached_rect);
    runtime.apply_todo_snapshot(
        &mut state,
        TodoSnapshotUpdate {
            title: "Plan".to_string(),
            items: vec![
                TodoSnapshotItem {
                    status: SharedTodoStatus::Completed,
                    content: "step 1".to_string(),
                },
                TodoSnapshotItem {
                    status: SharedTodoStatus::InProgress,
                    content: "step 2".to_string(),
                },
            ],
        },
    );
    assert!(state.plan_state.is_some());
    assert_eq!(
        state.render_state.cached_area,
        Some(cached_rect),
        "Some->Some (content-only) transition must NOT invalidate layout cache"
    );

    // Some -> None transition: cache must be invalidated.
    runtime.apply_todo_snapshot(
        &mut state,
        TodoSnapshotUpdate {
            title: String::new(),
            items: vec![],
        },
    );
    assert!(state.plan_state.is_none());
    assert!(
        state.render_state.cached_area.is_none(),
        "Some->None transition must invalidate layout cache (sidebar visibility changed)"
    );
}

#[test]
fn tool_update_preserves_previous_assistant_message() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    state
        .chat_state
        .messages
        .push(Message::assistant_streaming());
    runtime.stream_message_index = Some(0);
    runtime.set_stream_message_content(&mut state, "before tool", true);

    runtime.apply_tool_update(&mut state, sample_tool_update("call-1"), None);
    runtime.set_stream_message_content(&mut state, "after tool", true);

    assert_eq!(state.chat_state.messages.len(), 3);
    assert_eq!(state.chat_state.messages[0].role, MessageRole::Assistant);
    assert_eq!(state.chat_state.messages[0].content, "before tool");
    assert!(!state.chat_state.messages[0].is_streaming);

    let tool_state = state.chat_state.messages[1]
        .tool_state
        .as_ref()
        .expect("second message should be tool state");
    assert_eq!(tool_state.call_id, "call-1");

    assert_eq!(state.chat_state.messages[2].role, MessageRole::Assistant);
    assert_eq!(state.chat_state.messages[2].content, "after tool");
    assert!(state.chat_state.messages[2].is_streaming);
}

#[test]
fn thinking_stream_updates_active_assistant_message() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    runtime.set_stream_message_thinking_content(&mut state, "checking", true);
    runtime.set_stream_message_content(&mut state, "answer", true);

    assert_eq!(state.chat_state.messages.len(), 1);
    assert_eq!(state.chat_state.messages[0].thinking_content, "checking");
    assert_eq!(state.chat_state.messages[0].content, "answer");
    assert!(state.chat_state.messages[0].is_streaming);
}

#[test]
fn stream_updates_preserve_user_scroll_lock() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();
    state.chat_state.stick_to_bottom = false;

    let (tx, rx) = mpsc::unbounded_channel();
    runtime.stream_rx = Some(rx);
    tx.send(SessionTurnUpdate::SetAssistantThinking {
        agent_id: AgentId("cli-agent".to_string()),
        text: "checking".to_string(),
    })
    .expect("thinking update should send");
    tx.send(SessionTurnUpdate::SetAssistantContent {
        agent_id: AgentId("cli-agent".to_string()),
        text: "answer".to_string(),
    })
    .expect("content update should send");

    assert!(runtime.poll_stream_updates(&mut state));

    assert!(!state.chat_state.stick_to_bottom);
    assert_eq!(state.chat_state.messages.len(), 1);
    assert_eq!(state.chat_state.messages[0].thinking_content, "checking");
    assert_eq!(state.chat_state.messages[0].content, "answer");
}

#[test]
fn stream_updates_keep_existing_bottom_stickiness() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();
    state.chat_state.stick_to_bottom = true;

    let (tx, rx) = mpsc::unbounded_channel();
    runtime.stream_rx = Some(rx);
    tx.send(SessionTurnUpdate::SetAssistantContent {
        agent_id: AgentId("cli-agent".to_string()),
        text: "answer".to_string(),
    })
    .expect("content update should send");

    assert!(runtime.poll_stream_updates(&mut state));

    assert!(state.chat_state.stick_to_bottom);
    assert_eq!(state.chat_state.messages[0].content, "answer");
}

#[test]
fn stream_updates_surface_memory_state_transitions() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    let (tx, rx) = mpsc::unbounded_channel();
    runtime.stream_rx = Some(rx);
    tx.send(SessionTurnUpdate::MemoryStatus(MemoryStatus::Disabled))
        .expect("memory status update should send");
    assert!(runtime.poll_stream_updates(&mut state));
    assert_eq!(state.status_panel.memory_status, MemoryStatus::Disabled);

    tx.send(SessionTurnUpdate::MemoryStatus(MemoryStatus::Degraded))
        .expect("memory status update should send");
    assert!(runtime.poll_stream_updates(&mut state));
    assert_eq!(state.status_panel.memory_status, MemoryStatus::Degraded);

    tx.send(SessionTurnUpdate::MemoryStatus(MemoryStatus::Connected))
        .expect("memory status update should send");

    assert!(runtime.poll_stream_updates(&mut state));
    assert_eq!(state.status_panel.memory_status, MemoryStatus::Connected);
}

#[test]
fn background_memory_health_change_updates_status_without_a_turn() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();
    let (health_tx, health_rx) = watch::channel(MemoryAutomationHealth::Healthy);
    *runtime
        .session_gateway
        .memory_health
        .lock()
        .expect("memory health lock should not be poisoned") = Some(health_rx);

    health_tx
        .send(MemoryAutomationHealth::Degraded)
        .expect("memory health receiver should be present");

    assert!(runtime.poll_stream_updates(&mut state));
    assert_eq!(state.status_panel.memory_status, MemoryStatus::Degraded);
}

#[test]
fn child_stream_updates_create_subagent_lane_without_touching_root_messages() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    let (tx, rx) = mpsc::unbounded_channel();
    runtime.stream_rx = Some(rx);
    tx.send(SessionTurnUpdate::SetAssistantContent {
        agent_id: AgentId("child-agent".to_string()),
        text: "child answer".to_string(),
    })
    .expect("content update should send");

    assert!(runtime.poll_stream_updates(&mut state));

    assert!(state.chat_state.messages.is_empty());
    let lane = state
        .chat_state
        .subagent_lanes
        .get("child-agent")
        .expect("child lane should be created");
    assert_eq!(lane.messages.len(), 1);
    assert_eq!(lane.messages[0].content, "child answer");
    assert!(lane.messages[0].is_streaming);
}

#[test]
fn invalid_spawn_output_does_not_create_subagent_lane() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "spawn-1".to_string(),
            tool: "spawn_subagent".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "description": "Review code",
                "task_goal": "Find issues"
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: "not-json".to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        },
        Some("main".to_string()),
    );

    assert!(state.chat_state.subagent_lanes.is_empty());
    assert_eq!(state.chat_state.messages.len(), 1);
    assert!(state.chat_state.messages[0].tool_state.is_some());
}

#[test]
fn completed_spawn_output_creates_subagent_lane_with_metadata() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "spawn-2".to_string(),
            tool: "spawn_subagent".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "description": "Review code",
                "task_goal": "Find issues",
                "task_context": "Focus on tests"
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: r#"{"agent_id":"child-2"}"#.to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        },
        Some("main".to_string()),
    );

    let lane = state
        .chat_state
        .subagent_lanes
        .get("child-2")
        .expect("spawn should create lane");
    assert_eq!(lane.parent_agent_id.as_deref(), Some("main"));
    assert_eq!(lane.title, "Review code");
    assert_eq!(lane.description, "Review code");
    assert_eq!(lane.task_goal, "Find issues");
}

#[test]
fn remote_tool_result_does_not_clobber_subagent_spawn_metadata() {
    // Simulates the remote-mode event ordering: the daemon first forwards
    // a `SubagentSpawn` SSE event carrying full metadata, then a `ToolResult`
    // SSE event whose `args_preview` is stripped to an empty string. The
    // `ToolResult` must not overwrite the title/description/task_goal set
    // by `SubagentSpawn` with fallback "Subagent xxx" values.
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    // Step 1: simulate `SubagentSpawn` populating the lane with full
    // metadata (as the daemon-side `SubagentMetaComputingLoopSink` would).
    state.chat_state.ensure_subagent_lane(
        "child-3".to_string(),
        Some("root".to_string()),
        "Refactor module boundaries".to_string(),
        "Split the god module into focused crates".to_string(),
        " Land the split behind a feature flag".to_string(),
    );

    // Step 2: simulate the trailing `ToolResult` event with empty
    // `args_preview` (as `remote.rs` constructs it from the SSE event).
    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "spawn-3".to_string(),
            tool: "spawn_subagent".to_string(),
            summary: String::new(),
            args_preview: String::new(),
            command_preview: None,
            command: None,
            detail: r#"{"agent_id":"child-3"}"#.to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        },
        Some("root".to_string()),
    );

    let lane = state
        .chat_state
        .subagent_lanes
        .get("child-3")
        .expect("lane should still exist after ToolResult");
    assert_eq!(lane.title, "Refactor module boundaries");
    assert_eq!(lane.description, "Split the god module into focused crates");
    assert_eq!(lane.task_goal, " Land the split behind a feature flag");
}

#[test]
fn remote_turn_start_does_not_clobber_subagent_spawn_metadata() {
    // Simulates the remote-mode event ordering: the daemon first forwards
    // a `SubagentSpawn` SSE event carrying full metadata, then the child
    // agent's `TurnStart` arrives. The `TurnStart` handler must NOT call
    // `ensure_subagent_lane` with the generic "Subagent xxx" fallback
    // title (which would clobber the real title via `update_metadata`).
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    // Step 1: simulate `SubagentSpawn` populating the lane with full
    // metadata (as the daemon-side `SubagentMetaComputingLoopSink` would).
    state.chat_state.ensure_subagent_lane(
        "child-3".to_string(),
        Some("root".to_string()),
        "Refactor module boundaries".to_string(),
        "Split the god module into focused crates".to_string(),
        " Land the split behind a feature flag".to_string(),
    );

    // Step 2: feed a `TurnStart` event for the spawned child via the
    // stream channel, then drain it through `poll_stream_updates`.
    let (tx, rx) = mpsc::unbounded_channel::<SessionTurnUpdate>();
    runtime.stream_rx = Some(rx);
    tx.send(SessionTurnUpdate::TurnStart {
        agent_id: AgentId("child-3".to_string()),
        turn: 1,
    })
    .expect("send TurnStart");
    drop(tx);
    let changed = runtime.poll_stream_updates(&mut state);
    assert!(changed, "poll_stream_updates should report a change");

    let lane = state
        .chat_state
        .subagent_lanes
        .get("child-3")
        .expect("lane should still exist after TurnStart");
    assert_eq!(lane.title, "Refactor module boundaries");
    assert_eq!(lane.description, "Split the god module into focused crates");
    assert_eq!(lane.task_goal, " Land the split behind a feature flag");
    assert!(lane.is_running);
    assert_eq!(lane.last_turn, Some(1));
}

#[test]
fn remote_subagent_events_do_not_clobber_subagent_spawn_metadata() {
    // End-to-end verification that the generic "Subagent xxx" fallback
    // title passed by downstream event handlers (TurnStart,
    // SetAssistantContent, Tool) does not clobber the title/description/
    // task_goal populated earlier by a `SubagentSpawn` SSE event. Each
    // handler must use `ensure_subagent_lane_preserve_metadata` (or
    // equivalent) so the lane metadata set by `SubagentSpawn` survives
    // the child agent's entire turn.
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    // Step 1: simulate `SubagentSpawn` populating the lane with full
    // metadata (as the daemon-side `SubagentMetaComputingLoopSink` would).
    state.chat_state.ensure_subagent_lane(
        "child-4".to_string(),
        Some("root".to_string()),
        "Refactor module boundaries".to_string(),
        "Split the god module into focused crates".to_string(),
        " Land the split behind a feature flag".to_string(),
    );

    // Step 2: feed the child agent's turn lifecycle through the stream
    // channel: TurnStart → SetAssistantContent → Tool(Completed).
    let (tx, rx) = mpsc::unbounded_channel::<SessionTurnUpdate>();
    runtime.stream_rx = Some(rx);
    tx.send(SessionTurnUpdate::TurnStart {
        agent_id: AgentId("child-4".to_string()),
        turn: 1,
    })
    .expect("send TurnStart");
    tx.send(SessionTurnUpdate::SetAssistantContent {
        agent_id: AgentId("child-4".to_string()),
        text: "Working on the refactor...".to_string(),
    })
    .expect("send SetAssistantContent");
    tx.send(SessionTurnUpdate::Tool {
        agent_id: AgentId("child-4".to_string()),
        update: ToolExecutionUpdate {
            call_id: "child-4-call-1".to_string(),
            tool: "shell".to_string(),
            summary: "ls".to_string(),
            args_preview: String::new(),
            command_preview: None,
            command: None,
            detail: "src\ntests".to_string(),
            status: ToolExecutionStatus::Completed,
            exit_code: Some(0),
            duration_ms: Some(12),
            file_change: None,
        },
    })
    .expect("send Tool");
    drop(tx);

    // Drain all queued events.
    while runtime.poll_stream_updates(&mut state) {}

    // The metadata populated by `SubagentSpawn` must survive every
    // downstream event in the child's turn.
    let lane = state
        .chat_state
        .subagent_lanes
        .get("child-4")
        .expect("lane should still exist after child turn");
    assert_eq!(lane.title, "Refactor module boundaries");
    assert_eq!(lane.description, "Split the god module into focused crates");
    assert_eq!(lane.task_goal, " Land the split behind a feature flag");
}

#[test]
fn tool_update_drops_empty_streaming_placeholder() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    state
        .chat_state
        .messages
        .push(Message::assistant_streaming());
    runtime.stream_message_index = Some(0);

    runtime.apply_tool_update(&mut state, sample_tool_update("call-2"), None);

    assert_eq!(state.chat_state.messages.len(), 1);
    assert!(state.chat_state.messages[0].tool_state.is_some());
    assert!(runtime.stream_message_index.is_none());
}

#[test]
fn tool_update_tracks_session_file_changes_by_call_id() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");

    let mut state = AppState::new(PathBuf::from("config.toml"), workspace.clone())
        .expect("test app state should initialize");
    state.chat_state.messages.clear();

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "file_edit".to_string(),
            summary: String::new(),
            args_preview: "{\n  \"file_path\": \"src/main.rs\"\n}".to_string(),
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Running,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        },
        None,
    );

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "call-1".to_string(),
            tool: "file_edit".to_string(),
            summary: String::new(),
            args_preview: "{\n  \"file_path\": \"src/main.rs\"\n}".to_string(),
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: Some(crate::chat::FileChangeDelta {
                file_path: "src/main.rs".to_string(),
                additions: 2,
                deletions: 1,
            }),
        },
        None,
    );

    let stats = state
        .session_file_changes()
        .get("src/main.rs")
        .expect("file stats should be tracked");
    assert_eq!(stats.additions, 2);
    assert_eq!(stats.deletions, 1);
}

#[test]
fn completed_file_edit_without_running_update_tracks_args_delta() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");

    let mut state = AppState::new(PathBuf::from("config.toml"), workspace)
        .expect("test app state should initialize");
    state.chat_state.messages.clear();

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "call-args-only".to_string(),
            tool: "file_edit".to_string(),
            summary: String::new(),
            args_preview: serde_json::json!({
                "file_path": "README.md",
                "old_string": "[@shen](https://github.com/shen)",
                "new_string": "[@hypo](https://github.com/hypo)"
            })
            .to_string(),
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: None,
        },
        None,
    );

    let stats = state
        .session_file_changes()
        .get("README.md")
        .expect("file stats should be tracked from args");
    assert_eq!(stats.additions, 1);
    assert_eq!(stats.deletions, 1);
}

#[test]
fn finish_stream_done_clears_per_call_state_but_preserves_file_totals() {
    // Regression for remote-mode memory leak: the SSE stream emits a
    // `Done` event but no `LoopEnd` (the daemon's `SseLoopEventSink`
    // only stores the summary without forwarding it). Without this
    // cleanup in `finish_stream_done`, `tool_file_changes` /
    // `tool_file_baselines` would grow unboundedly across turns. The
    // per-file totals (`session_file_changes`) must survive so the diff
    // panel keeps showing the session's cumulative changes.
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace dir");

    let mut state = AppState::new(PathBuf::from("config.toml"), workspace)
        .expect("test app state should initialize");
    state.chat_state.messages.clear();

    runtime.apply_tool_update(
        &mut state,
        ToolExecutionUpdate {
            call_id: "remote-call-1".to_string(),
            tool: "file_edit".to_string(),
            summary: String::new(),
            args_preview: "{\n  \"file_path\": \"src/main.rs\"\n}".to_string(),
            command_preview: None,
            command: None,
            detail: String::new(),
            status: ToolExecutionStatus::Completed,
            exit_code: None,
            duration_ms: None,
            file_change: Some(crate::chat::FileChangeDelta {
                file_path: "src/main.rs".to_string(),
                additions: 2,
                deletions: 1,
            }),
        },
        None,
    );

    runtime.finish_stream_done(
        &mut state,
        PendingStreamDone {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
            estimated_input_tokens: 0,
            messages: Vec::new(),
        },
    );

    // Per-file totals survive `finish_stream_done`.
    let stats = state
        .session_file_changes()
        .get("src/main.rs")
        .expect("file totals should survive finish_stream_done");
    assert_eq!(stats.additions, 2);
    assert_eq!(stats.deletions, 1);
}

#[test]
fn first_token_latency_is_recorded_once_and_completion_uses_reported_prompt_tokens() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    state
        .chat_state
        .messages
        .push(Message::assistant_streaming());
    runtime.stream_message_index = Some(0);
    runtime.request_start = Some(Instant::now() - Duration::from_millis(20));

    runtime.set_stream_message_content(&mut state, "H", true);
    runtime.record_first_token_latency_if_needed(&mut state);
    let first_token_latency_ms = state.status_panel.last_latency_ms;
    assert!(first_token_latency_ms >= 20);
    assert!(runtime.first_token_latency_recorded);

    runtime.request_start = Some(Instant::now() - Duration::from_millis(80));
    runtime.record_first_token_latency_if_needed(&mut state);
    assert_eq!(state.status_panel.last_latency_ms, first_token_latency_ms);

    runtime.finish_stream_done(
        &mut state,
        PendingStreamDone {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 42,
            estimated_input_tokens: 18,
            messages: Vec::new(),
        },
    );

    assert_eq!(state.status_panel.last_latency_ms, first_token_latency_ms);
    assert_eq!(state.status_panel.prompt_tokens, 10);
    assert_eq!(state.status_panel.completion_tokens, 5);
    assert_eq!(state.status_panel.input_context_tokens, 10);
    assert!(!state.status_panel.input_context_tokens_estimated);
}

#[test]
fn completion_accumulates_usage_totals_across_turns_without_changing_ctx_semantics() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    state
        .chat_state
        .messages
        .push(Message::assistant_streaming());
    runtime.stream_message_index = Some(0);
    runtime.request_start = Some(Instant::now());

    runtime.finish_stream_done(
        &mut state,
        PendingStreamDone {
            prompt_tokens: 10,
            completion_tokens: 5,
            total_tokens: 15,
            estimated_input_tokens: 18,
            messages: Vec::new(),
        },
    );

    state
        .chat_state
        .messages
        .push(Message::assistant_streaming());
    runtime.stream_message_index = Some(1);
    runtime.request_start = Some(Instant::now());

    runtime.finish_stream_done(
        &mut state,
        PendingStreamDone {
            prompt_tokens: 25,
            completion_tokens: 7,
            total_tokens: 32,
            estimated_input_tokens: 31,
            messages: Vec::new(),
        },
    );

    assert_eq!(state.status_panel.prompt_tokens, 35);
    assert_eq!(state.status_panel.completion_tokens, 12);
    assert_eq!(state.status_panel.total_tokens, 47);
    assert_eq!(state.status_panel.input_context_tokens, 25);
    assert!(!state.status_panel.input_context_tokens_estimated);
}

#[test]
fn completion_falls_back_to_estimated_input_tokens_when_prompt_usage_is_missing() {
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let mut state = test_state();

    state
        .chat_state
        .messages
        .push(Message::assistant_streaming());
    runtime.stream_message_index = Some(0);
    runtime.request_start = Some(Instant::now());

    runtime.finish_stream_done(
        &mut state,
        PendingStreamDone {
            prompt_tokens: 0,
            completion_tokens: 6,
            total_tokens: 30,
            estimated_input_tokens: 24,
            messages: Vec::new(),
        },
    );

    assert_eq!(state.status_panel.prompt_tokens, 0);
    assert_eq!(state.status_panel.completion_tokens, 6);
    assert_eq!(state.status_panel.total_tokens, 6);
    assert_eq!(state.status_panel.input_context_tokens, 24);
    assert!(state.status_panel.input_context_tokens_estimated);
}
