use super::*;

#[test]
fn cancelled_event_serializes_runtime_id() {
    let value = serde_json::to_value(SseStreamEvent::Cancelled {
        session_id: "runtime-1".to_string(),
    })
    .expect("event should serialize");

    assert_eq!(value["runtime_id"], "runtime-1");
    assert!(value.get("session_id").is_none());
}

#[test]
fn tool_call_event_serializes_snake_case_status() {
    let value = serde_json::to_value(SseStreamEvent::ToolCall {
        agent_id: "child-1".to_string(),
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        args_preview: "{}".to_string(),
        status: ToolCallStatus::Running,
        detail: String::new(),
    })
    .expect("event should serialize");

    assert_eq!(value["type"], "tool_call");
    assert_eq!(value["status"], "running");
    assert_eq!(value["agent_id"], "child-1");
}

#[test]
fn loop_end_event_carries_agent_id() {
    let value = serde_json::to_value(SseStreamEvent::LoopEnd {
        agent_id: "child-1".to_string(),
        turn_count: 3,
        total_tokens: 1024,
        stop_reason: "end_turn".to_string(),
    })
    .expect("event should serialize");

    assert_eq!(value["type"], "loop_end");
    assert_eq!(value["agent_id"], "child-1");
    assert_eq!(value["turn_count"], 3);
    assert_eq!(value["total_tokens"], 1024);
    assert_eq!(value["stop_reason"], "end_turn");
}

#[test]
fn tool_result_event_carries_args_preview() {
    let value = serde_json::to_value(SseStreamEvent::ToolResult {
        agent_id: "root".to_string(),
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        output_preview: "done".to_string(),
        is_error: false,
        args_preview: "{\"command\":\"ls\"}".to_string(),
    })
    .expect("event should serialize");

    assert_eq!(value["type"], "tool_result");
    assert_eq!(value["args_preview"], "{\"command\":\"ls\"}");
}

/// Inner sink should still receive the event after the wrapper
/// forwards it to SSE.
#[test]
fn sse_tool_event_sink_delegates_to_inner() {
    #[derive(Default)]
    struct Recorder {
        seen: Mutex<Vec<ToolLifecycleEvent>>,
    }
    impl ToolEventSink for Recorder {
        fn emit(&self, event: ToolLifecycleEvent) {
            self.seen.lock().unwrap().push(event);
        }
    }

    let (tx, _rx) = mpsc::unbounded_channel::<SseStreamEvent>();
    let recorder = Arc::new(Recorder::default());
    let sink = SseToolEventSink::with_inner(tx, recorder.clone() as Arc<dyn ToolEventSink>);
    sink.emit(ToolLifecycleEvent::Running {
        call_id: "call-1".to_string(),
        tool_name: "bash".to_string(),
        args_preview: "{}".to_string(),
    });

    let seen = recorder.seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert!(matches!(seen[0], ToolLifecycleEvent::Running { .. }));
}

/// Drives a subagent lifecycle sequence (`tool_call(running)` ->
/// `tool_call(completed)` [skipped: terminal] -> `loop_end`)
/// through `SseToolEventSink` + `SseLoopEventSink` and asserts the
/// SSE channel receives the events in order. Terminal lifecycle
/// events (`Completed`) are NOT forwarded as `ToolCall` SSE events
/// — the subsequent `ToolResult` SSE event conveys the terminal
/// state, avoiding duplicate updates.
#[tokio::test]
async fn sse_subagent_lifecycle_events_reach_channel_in_order() {
    let (tx, mut rx) = mpsc::unbounded_channel::<SseStreamEvent>();
    // Inner sink is a no-op; diff-tracker delegation is covered
    // separately by `sse_tool_event_sink_delegates_to_inner`.
    struct NoopSink;
    impl ToolEventSink for NoopSink {
        fn emit(&self, _event: ToolLifecycleEvent) {}
    }

    let loop_sink = SseLoopEventSink::new(tx.clone());
    let tool_sink = SseToolEventSink::with_inner(tx, Arc::new(NoopSink) as Arc<dyn ToolEventSink>);

    // tool call (running) -> tool call (completed, skipped) -> loop end
    // mirrors a real subagent turn on the wire.
    tool_sink.emit(
        ToolLifecycleEvent::Running {
            call_id: "child-call-1".to_string(),
            tool_name: "bash".to_string(),
            args_preview: r#"{"command":"ls"}"#.to_string(),
        }
        .scoped(AgentId("child-1".to_string())),
    );
    tool_sink.emit(
        ToolLifecycleEvent::Completed {
            call_id: "child-call-1".to_string(),
            tool_name: "bash".to_string(),
            args_preview: r#"{"command":"ls"}"#.to_string(),
        }
        .scoped(AgentId("child-1".to_string())),
    );
    loop_sink.on_loop_end(
        &AgentId("child-1".to_string()),
        &LoopEndSummary {
            turn_count: 1,
            total_tokens: 0,
            stop_reason: "end_turn".to_string(),
        },
    );

    // Drain the channel and assert the wire-format sequence.
    let mut received = Vec::new();
    while let Ok(event) = rx.try_recv() {
        received.push(event);
    }
    // Expect: tool_call(running) + loop_end. The `Completed`
    // lifecycle event is NOT forwarded as SSE (terminal state is
    // conveyed by the subsequent `ToolResult` SSE event).
    assert_eq!(
        received.len(),
        2,
        "expected running + loop_end (completed is skipped); got {received:?}"
    );
    assert!(matches!(
        received[0],
        SseStreamEvent::ToolCall {
            ref agent_id,
            status: ToolCallStatus::Running,
            ..
        } if agent_id == "child-1"
    ));
    assert!(matches!(
        received[1],
        SseStreamEvent::LoopEnd {
            ref agent_id,
            turn_count: 1,
            total_tokens: 0,
            ref stop_reason,
        } if agent_id == "child-1" && stop_reason == "end_turn"
    ));
}
