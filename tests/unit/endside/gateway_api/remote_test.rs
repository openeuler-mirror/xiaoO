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
fn parses_loop_end_event_without_summary_fields() {
    // Older daemons send only `agent_id`.
    let parsed =
        parse_sse_frame("event: loop_end\ndata: {\"type\":\"loop_end\",\"agent_id\":\"child-1\"}")
            .expect("parse")
            .expect("event");

    match parsed {
        RemoteSseEvent::LoopEnd { agent_id } => {
            assert_eq!(agent_id, "child-1");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

/// Forward-compat: newer daemons attach summary fields
/// (`turn_count` / `total_tokens` / `stop_reason`) to `loop_end`. The TUI
/// reads the summary via its own `loop_summary` sink, so these are unknown
/// fields on `RemoteSseEvent::LoopEnd` and must be silently ignored rather
/// than break deserialization.
#[test]
fn parses_loop_end_event_with_summary_fields_ignoring_them() {
    let parsed = parse_sse_frame(
        "event: loop_end\ndata: {\"type\":\"loop_end\",\"agent_id\":\"child-1\",\"turn_count\":2,\"total_tokens\":512,\"stop_reason\":\"end_turn\"}",
    )
    .expect("parse")
    .expect("event");

    match parsed {
        RemoteSseEvent::LoopEnd { agent_id } => {
            assert_eq!(agent_id, "child-1");
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

// ── Same-config verification at /remote connect time ───────────────────────
//
// Decision paths: digests match (allow) / differ (reject), old daemon
// without identity (allow), missing local identity (reject — fail
// closed), unparsable body (reject — fail closed).

fn temp_config_with(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, content).expect("write temp config");
    (dir, path)
}

fn health_body_with_sha256(sha256: &str) -> String {
    format!(
        r#"{{"status":"ok","version":"0.1.0","config_path":"/etc/xiaoo/config.toml","config_sha256":"{sha256}"}}"#
    )
}

/// Health body of a daemon that reports both its config digest and the
/// identity of the machine it runs on.
fn health_body_with_sha256_and_host(sha256: &str, host_identity: &str) -> String {
    format!(
        r#"{{"status":"ok","version":"0.1.0","config_path":"/etc/xiaoo/config.toml","config_sha256":"{sha256}","host_identity":"{host_identity}"}}"#
    )
}

/// Daemon digest matches the local config → connect succeeds.
#[tokio::test]
async fn connect_remote_accepts_matching_config() {
    let (_dir, config_path) = temp_config_with("[llm]\nmodel = \"m\"\n");
    let identity =
        xiaoo_shared::config_identity::ConfigFileIdentity::from_path(&config_path).unwrap();
    let mut server = mockito::Server::new_async().await;
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(health_body_with_sha256(&identity.sha256))
        .create_async()
        .await;

    let mut state = AppState::new(config_path.clone(), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    runtime
        .connect_remote(&mut state, server.url(), None)
        .await
        .expect("matching config should connect");
    assert_eq!(runtime.remote_base_url(), Some(server.url().as_str()));
}

/// Digest mismatch → refused, with paths, digest prefixes and restart
/// guidance in the error.
#[tokio::test]
async fn connect_remote_rejects_config_mismatch() {
    let (_dir, config_path) = temp_config_with("[llm]\nmodel = \"m\"\n");
    let mut server = mockito::Server::new_async().await;
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(health_body_with_sha256("0000deadbeef"))
        .create_async()
        .await;

    let mut state = AppState::new(config_path.clone(), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let error = runtime
        .connect_remote(&mut state, server.url(), None)
        .await
        .expect_err("config mismatch must be rejected");

    assert!(
        error.starts_with("config mismatch:"),
        "error should name the mismatch, got: {error}"
    );
    assert!(error.contains("must use the same config file"));
    assert!(error.contains("/etc/xiaoo/config.toml"));
    assert!(
        error.contains(config_path.display().to_string().as_str()),
        "error should show the local config path"
    );
    assert!(
        error.contains("(sha256 0000dead"),
        "error should show the daemon digest prefix for diffing, got: {error}"
    );
    assert!(
        error.contains("restart the daemon"),
        "error should explain that the daemon holds a startup snapshot, got: {error}"
    );
    assert!(
        runtime.remote_base_url().is_none(),
        "refusal must not leave remote config behind"
    );
}

/// Health response from an old daemon without identity fields → allowed.
#[tokio::test]
async fn connect_remote_allows_daemon_without_config_identity() {
    let (_dir, config_path) = temp_config_with("[llm]\nmodel = \"m\"\n");
    let mut server = mockito::Server::new_async().await;
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(r#"{"status":"ok","version":"0.0.9"}"#)
        .create_async()
        .await;

    let mut state =
        AppState::new(config_path, PathBuf::from(".")).expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    runtime
        .connect_remote(&mut state, server.url(), None)
        .await
        .expect("old daemons without config identity must still connect");
}

/// Unreadable local config file at startup → fail closed.
#[tokio::test]
async fn connect_remote_fails_closed_when_local_config_unreadable() {
    let dir = tempfile::tempdir().expect("temp dir");
    let missing = dir.path().join("deleted-config.toml");
    let mut server = mockito::Server::new_async().await;
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(health_body_with_sha256("0000deadbeef"))
        .create_async()
        .await;

    let mut state = AppState::new(missing.clone(), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let error = runtime
        .connect_remote(&mut state, server.url(), None)
        .await
        .expect_err("unreadable local config must fail closed");
    assert!(
        error.contains("cannot verify config match"),
        "error should explain the failure, got: {error}"
    );
    assert!(
        runtime.remote_base_url().is_none(),
        "refusal must not leave remote config behind"
    );
}

/// Daemon reporting a different host → refused before entering remote
/// mode, even though the config digests match. Machines where no local
/// identity can be resolved take the fail-closed branch instead — both
/// outcomes must reject the connect.
#[tokio::test]
async fn connect_remote_rejects_daemon_on_different_host() {
    let (_dir, config_path) = temp_config_with("[llm]\nmodel = \"m\"\n");
    let identity =
        xiaoo_shared::config_identity::ConfigFileIdentity::from_path(&config_path).unwrap();
    let mut server = mockito::Server::new_async().await;
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(health_body_with_sha256_and_host(
            &identity.sha256,
            "other-machine",
        ))
        .create_async()
        .await;

    let mut state = AppState::new(config_path.clone(), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    let error = runtime
        .connect_remote(&mut state, server.url(), None)
        .await
        .expect_err("daemon on another host must be rejected");
    match xiaoo_shared::host_identity::local_host_identity() {
        Some(local) => {
            assert!(
                error.starts_with("daemon runs on a different host"),
                "error should name the host mismatch, got: {error}"
            );
            assert!(
                error.contains(&format!("local={local}")),
                "error should show the local identity, got: {error}"
            );
            assert!(
                error.contains("daemon=other-machine"),
                "error should show the daemon identity, got: {error}"
            );
        }
        None => assert!(
            error.contains("cannot verify same-host"),
            "unresolvable local identity must fail closed, got: {error}"
        ),
    }
    assert!(
        runtime.remote_base_url().is_none(),
        "refusal must not leave remote config behind"
    );
}

/// Daemon reporting the local machine's identity → connects. Skipped on
/// machines where no local identity can be resolved at all: that branch
/// is covered by the fail-closed case above.
#[tokio::test]
async fn connect_remote_accepts_daemon_on_same_host() {
    let Some(local) = xiaoo_shared::host_identity::local_host_identity() else {
        eprintln!("skipping: no local host identity on this machine");
        return;
    };
    let (_dir, config_path) = temp_config_with("[llm]\nmodel = \"m\"\n");
    let identity =
        xiaoo_shared::config_identity::ConfigFileIdentity::from_path(&config_path).unwrap();
    let mut server = mockito::Server::new_async().await;
    let _health = server
        .mock("GET", "/api/v1/health")
        .with_status(200)
        .with_header("content-type", "application/json")
        .with_body(health_body_with_sha256_and_host(&identity.sha256, &local))
        .create_async()
        .await;

    let mut state = AppState::new(config_path.clone(), PathBuf::from("."))
        .expect("test app state should initialize");
    let mut runtime = GatewayRuntime::new(uuid::Uuid::new_v4().to_string());
    runtime
        .connect_remote(&mut state, server.url(), None)
        .await
        .expect("same-host daemon with matching config should connect");
    assert_eq!(runtime.remote_base_url(), Some(server.url().as_str()));
}

/// Pure-function decision table for all verification paths.
#[test]
fn verify_remote_config_match_decision_table() {
    use super::verify_remote_config_match;
    let (_dir, config_path) = temp_config_with("[llm]\nmodel = \"m\"\n");
    let identity =
        xiaoo_shared::config_identity::ConfigFileIdentity::from_path(&config_path).unwrap();

    // Matching digest → Ok.
    verify_remote_config_match(
        Some(&identity),
        &config_path,
        &health_body_with_sha256(&identity.sha256),
    )
    .expect("matching digest should pass");

    // Mismatching digest → Err naming both paths.
    let error = verify_remote_config_match(
        Some(&identity),
        &config_path,
        &health_body_with_sha256("abc123"),
    )
    .expect_err("mismatching digest must fail");
    assert!(error.contains("config mismatch"));
    assert!(error.contains("/etc/xiaoo/config.toml"));

    // Old daemon without identity fields → Ok.
    verify_remote_config_match(
        Some(&identity),
        &config_path,
        r#"{"status":"ok","version":"0.0.9"}"#,
    )
    .expect("old daemon must pass");

    // Unparsable health body → Err (fail closed: every daemon version
    // returns JSON, so this means a proxy or another service at the URL).
    let error = verify_remote_config_match(Some(&identity), &config_path, "not-json")
        .expect_err("unparsable health body must fail closed");
    assert!(
        error.contains("not the expected health JSON"),
        "error should name the parse failure, got: {error}"
    );
    assert!(
        error.contains("not-json"),
        "error should quote the body preview for diagnosis, got: {error}"
    );

    // Missing startup identity → Err (fail closed).
    let missing = PathBuf::from("/nonexistent/endside-test-config.toml");
    let error = verify_remote_config_match(None, &missing, &health_body_with_sha256("abc123"))
        .expect_err("missing startup identity must fail closed");
    assert!(error.contains("cannot verify config match"));
}

/// Decision table for the same-host constraint on its own, with both
/// identities as inputs (the caller resolves the local one).
#[test]
fn verify_same_host_decision_table() {
    use super::verify_same_host;

    // Daemon reports the local identity → Ok.
    verify_same_host(Some("host-a"), Some("host-a")).expect("same host should pass");

    // Different identity → Err naming both sides and the remedy.
    let error = verify_same_host(Some("host-a"), Some("host-b"))
        .expect_err("different host must be rejected");
    assert!(
        error.starts_with("daemon runs on a different host"),
        "error should name the host mismatch, got: {error}"
    );
    assert!(
        error.contains("local=host-b") && error.contains("daemon=host-a"),
        "error should show both identities, got: {error}"
    );
    assert!(
        error.contains("xGovernor"),
        "error should point at the multi-node remedy, got: {error}"
    );

    // Old daemon without host_identity → Ok regardless of local resolution.
    verify_same_host(None, Some("host-b")).expect("old daemon must pass");
    verify_same_host(None, None).expect("old daemon must pass without local identity");

    // Daemon reports an identity but local resolution failed → fail closed.
    let error = verify_same_host(Some("host-a"), None)
        .expect_err("unresolvable local identity must fail closed");
    assert!(
        error.contains("cannot verify same-host"),
        "error should explain the fail-closed path, got: {error}"
    );
}
