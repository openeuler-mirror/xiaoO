use std::path::PathBuf;
use tokio::sync::watch;

use reqwest::StatusCode;

use crate::app_state::AppState;
use crate::gateway::MemoryAutomationHealth;
use crate::status_panel::MemoryStatus;

use super::{parse_sse_frame, take_sse_frame, GatewayRuntime, RemoteSseEvent};

#[test]
fn configuring_remote_marks_memory_state_unknown() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());

    runtime.configure_remote(&mut state, "http://daemon.example".to_string(), None);

    assert_eq!(state.status_panel.memory_status, MemoryStatus::Unknown);
}

#[tokio::test]
async fn disconnecting_remote_restores_cached_local_memory_health() {
    let mut state = AppState::new(PathBuf::from("config.toml"), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let (_health_tx, health_rx) = watch::channel(MemoryAutomationHealth::Healthy);
    *runtime
        .session_gateway
        .memory_health
        .lock()
        .expect("memory health lock should not be poisoned") = Some(health_rx);
    runtime.configure_remote(&mut state, "http://daemon.example".to_string(), None);

    runtime
        .disconnect_remote(&mut state)
        .await
        .expect("remote disconnect should succeed");

    assert_eq!(state.status_panel.memory_status, MemoryStatus::Connected);
}

#[test]
fn parses_sse_frame_from_split_buffer() {
    let mut buffer = String::from(
        "event: text_delta\ndata: {\"type\":\"text_delta\",\"delta\":\"he\",\"snapshot\":\"he\"}\n\nrest",
    );
    let frame = take_sse_frame(&mut buffer).expect("frame");
    let parsed = parse_sse_frame(&frame).expect("parse").expect("event");
    match parsed {
        RemoteSseEvent::TextDelta {
            agent_id,
            delta,
            snapshot,
        } => {
            assert!(agent_id.is_none());
            assert_eq!(delta, "he");
            assert_eq!(snapshot, "he");
        }
        other => panic!("unexpected event: {other:?}"),
    }
    assert_eq!(buffer, "rest");
}

#[test]
fn parses_sse_frame_with_agent_id() {
    let parsed = parse_sse_frame(
        "event: text_delta\ndata: {\"type\":\"text_delta\",\"agent_id\":\"child\",\"delta\":\"he\",\"snapshot\":\"he\"}",
    )
    .expect("parse")
    .expect("event");

    match parsed {
        RemoteSseEvent::TextDelta { agent_id, .. } => {
            assert_eq!(agent_id.as_deref(), Some("child"));
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn ignores_keepalive_frame() {
    let parsed = parse_sse_frame(": keepalive").expect("parse");
    assert!(parsed.is_none());
}

/// Unknown event types must not kill the SSE stream; the frame is
/// skipped via `Ok(None)`.
#[test]
fn ignores_unknown_event_type_instead_of_killing_stream() {
    let parsed = parse_sse_frame(
        "event: future_event\ndata: {\"type\":\"future_event\",\"payload\":\"...\"}",
    )
    .expect("parsing an unknown event type must not be a hard error");
    assert!(parsed.is_none(), "unknown event types should be skipped");
}

#[test]
fn parses_tool_call_running_event() {
    let parsed = parse_sse_frame(
        "event: tool_call\ndata: {\"type\":\"tool_call\",\"agent_id\":\"child-1\",\"call_id\":\"call-1\",\"tool_name\":\"bash\",\"args_preview\":\"{}\",\"status\":\"running\",\"detail\":\"\"}",
    )
    .expect("parse")
    .expect("event");

    match parsed {
        RemoteSseEvent::ToolCall {
            agent_id,
            call_id,
            tool_name,
            args_preview,
            status,
            detail,
        } => {
            assert_eq!(agent_id, "child-1");
            assert_eq!(call_id, "call-1");
            assert_eq!(tool_name, "bash");
            assert_eq!(args_preview, "{}");
            assert_eq!(status, super::ToolCallStatus::Running);
            assert!(detail.is_empty());
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn parses_loop_end_event() {
    // Backward-compat: older daemons omit the summary fields
    // (each is `#[serde(default)]`).
    let parsed =
        parse_sse_frame("event: loop_end\ndata: {\"type\":\"loop_end\",\"agent_id\":\"child-1\"}")
            .expect("parse")
            .expect("event");

    match parsed {
        RemoteSseEvent::LoopEnd {
            agent_id,
            turn_count,
            total_tokens,
            stop_reason,
        } => {
            assert_eq!(agent_id, "child-1");
            assert_eq!(turn_count, 0);
            assert_eq!(total_tokens, 0);
            assert!(stop_reason.is_empty());
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

/// Forward-compat: a newer daemon includes the summary fields on
/// `loop_end` and the TUI should populate them.
#[test]
fn parses_loop_end_with_summary() {
    let parsed = parse_sse_frame(
        "event: loop_end\ndata: {\"type\":\"loop_end\",\"agent_id\":\"child-1\",\"turn_count\":2,\"total_tokens\":512,\"stop_reason\":\"end_turn\"}",
    )
    .expect("parse")
    .expect("event");

    match parsed {
        RemoteSseEvent::LoopEnd {
            agent_id,
            turn_count,
            total_tokens,
            stop_reason,
        } => {
            assert_eq!(agent_id, "child-1");
            assert_eq!(turn_count, 2);
            assert_eq!(total_tokens, 512);
            assert_eq!(stop_reason, "end_turn");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

/// Backward-compat: older daemons omit `args_preview` on
/// `tool_result` (the field is `#[serde(default)]`).
#[test]
fn parses_tool_result_without_args_preview() {
    let parsed = parse_sse_frame(
        "event: tool_result\ndata: {\"type\":\"tool_result\",\"agent_id\":\"root\",\"call_id\":\"call-1\",\"tool_name\":\"bash\",\"output_preview\":\"done\",\"is_error\":false}",
    )
    .expect("parse")
    .expect("event");

    match parsed {
        RemoteSseEvent::ToolResult { args_preview, .. } => {
            assert!(args_preview.is_empty());
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

/// Forward-compat: a newer daemon includes `args_preview` on
/// `tool_result` and the TUI should populate the field.
#[test]
fn parses_tool_result_with_args_preview() {
    let parsed = parse_sse_frame(
        "event: tool_result\ndata: {\"type\":\"tool_result\",\"agent_id\":\"root\",\"call_id\":\"call-1\",\"tool_name\":\"bash\",\"output_preview\":\"done\",\"is_error\":false,\"args_preview\":\"{\\\"command\\\":\\\"ls\\\"}\"}",
    )
    .expect("parse")
    .expect("event");

    match parsed {
        RemoteSseEvent::ToolResult { args_preview, .. } => {
            assert_eq!(args_preview, "{\"command\":\"ls\"}");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

#[test]
fn permanent_rpc_error_classification() {
    use super::{is_permanent_rpc_error, PostJsonError, POST_JSON_SAFETY_TIMEOUT};
    // Transport errors are transient — worth retrying.
    assert!(!is_permanent_rpc_error(&PostJsonError::Transport(
        "connection refused".to_string()
    )));
    assert!(!is_permanent_rpc_error(&PostJsonError::Timeout {
        path: "/api/v1/runtimes/cancel".to_string(),
        timeout: POST_JSON_SAFETY_TIMEOUT,
    }));
    // 5xx are transient.
    assert!(!is_permanent_rpc_error(&PostJsonError::Http {
        status: StatusCode::BAD_GATEWAY,
        body: "upstream unavailable".to_string(),
    }));
    // Rate limiting and request timeouts are transient.
    assert!(!is_permanent_rpc_error(&PostJsonError::Http {
        status: StatusCode::TOO_MANY_REQUESTS,
        body: String::new(),
    }));
    assert!(!is_permanent_rpc_error(&PostJsonError::Http {
        status: StatusCode::REQUEST_TIMEOUT,
        body: String::new(),
    }));
    // Definitive 4xx rejections are permanent — retrying cannot help.
    assert!(is_permanent_rpc_error(&PostJsonError::Http {
        status: StatusCode::NOT_FOUND,
        body: "{\"error\":\"session not found: s1\"}".to_string(),
    }));
    assert!(is_permanent_rpc_error(&PostJsonError::Http {
        status: StatusCode::CONFLICT,
        body: "session attached by another client".to_string(),
    }));
    assert!(is_permanent_rpc_error(&PostJsonError::Http {
        status: StatusCode::UNAUTHORIZED,
        body: String::new(),
    }));
}

/// `PostJsonError`'s `Display` shapes feed logs and caller-facing
/// strings; lock them so a refactor cannot silently change them.
#[test]
fn post_json_error_display_shapes() {
    use super::PostJsonError;
    assert_eq!(
        PostJsonError::Transport("connection refused".to_string()).to_string(),
        "connection refused"
    );
    assert_eq!(
        PostJsonError::Timeout {
            path: "/api/v1/runtimes/cancel".to_string(),
            timeout: std::time::Duration::from_secs(10),
        }
        .to_string(),
        "HTTP POST /api/v1/runtimes/cancel timed out after 10s"
    );
    assert_eq!(
        PostJsonError::Http {
            status: StatusCode::NOT_FOUND,
            body: "{\"error\":\"session not found: s1\"}".to_string(),
        }
        .to_string(),
        "HTTP 404 Not Found {\"error\":\"session not found: s1\"}"
    );
}
