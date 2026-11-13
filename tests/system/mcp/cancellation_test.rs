//! End-to-end tests for MCP tool-call cancellation over stdio.
//!
//! Reproduces the gap tracked in issue #184: cancelling a turn used to leave
//! the MCP request running until the tool finished, and the late result was
//! still consumed. These tests drive a real stdio server that keeps reading
//! stdin while a slow call is in flight, so both halves of the contract are
//! observable:
//!
//! 1. the client sends `notifications/cancelled` for the in-flight request;
//! 2. the client stops waiting immediately and drops the late response.
//!
//! Requires `python3` (the same requirement as `stdio_handshake_test.rs`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use mcp::{McpClient, McpError, McpServerConfig, Transport};
use tokio_util::sync::CancellationToken;

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests")
        .join("system")
        .join("mcp")
        .join("fixtures")
        .join("mock_cancel_server_test.py")
}

fn server_config(log_path: &Path) -> McpServerConfig {
    let mut env = HashMap::new();
    env.insert(
        "MOCK_CANCEL_LOG".to_string(),
        log_path.to_string_lossy().into_owned(),
    );
    McpServerConfig {
        name: "mock-cancel".to_string(),
        transport: Transport::Stdio,
        command: Some("python3".to_string()),
        args: vec![script_path().to_string_lossy().into_owned()],
        env,
        url: None,
        bearer_token_env: None,
        agent_id: None,
        headers: std::collections::BTreeMap::new(),
        enabled: Some(true),
        timeout_ms: 10_000,
        effect: mcp::EffectSection::default(),
    }
}

/// Poll the event log until `needle` appears. Returns the full log so the
/// caller can make further assertions.
async fn wait_for_log(log_path: &Path, needle: &str, budget: Duration) -> String {
    let deadline = Instant::now() + budget;
    loop {
        if let Ok(content) = tokio::fs::read_to_string(log_path).await {
            if content.contains(needle) {
                return content;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {needle:?} in {}",
            log_path.display()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test]
async fn cancellation_notifies_server_and_drops_late_response() {
    assert!(
        python_available(),
        "python3 not available on PATH; mcp cancellation test requires python3"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let log_path = dir.path().join("cancel.log");
    let client = McpClient::connect(&server_config(&log_path))
        .await
        .expect("connect mock cancel server");
    client.initialize().await.expect("initialize");

    let tools = client.list_tools().await.expect("list tools");
    assert!(tools.iter().any(|tool| tool.name == "slow"));

    let token = CancellationToken::new();
    let canceller = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        canceller.cancel();
    });

    let started = Instant::now();
    let error = client
        .call_tool_with_cancel("slow", serde_json::json!({"delay_ms": 3000}), Some(&token))
        .await
        .expect_err("cancelled call must not return a result");
    let elapsed = started.elapsed();

    assert!(
        matches!(error, McpError::Cancelled),
        "expected McpError::Cancelled, got {error:?}"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "cancelled call waited for the tool: {elapsed:?}"
    );

    let log = wait_for_log(&log_path, "\"cancelled\"", Duration::from_secs(2)).await;
    assert!(
        log.contains("\"requestId\""),
        "cancellation notification carried no requestId: {log}"
    );

    // The mock server deliberately ignores the cancellation and writes the
    // late response anyway. Dropping it must not affect the next request.
    wait_for_log(&log_path, "late_response_sent", Duration::from_secs(4)).await;
    let echo = client
        .call_tool("echo", serde_json::json!({"message": "after-cancel"}))
        .await
        .expect("client must stay usable after a cancelled call");
    assert_eq!(echo.flatten_text(), "echo:after-cancel");
}

#[tokio::test]
async fn already_cancelled_token_does_not_block() {
    assert!(
        python_available(),
        "python3 not available on PATH; mcp cancellation test requires python3"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let client = McpClient::connect(&server_config(&dir.path().join("no-cancel.log")))
        .await
        .expect("connect mock cancel server");
    client.initialize().await.expect("initialize");

    let token = CancellationToken::new();
    token.cancel();

    let started = Instant::now();
    let error = client
        .call_tool_with_cancel("slow", serde_json::json!({"delay_ms": 3000}), Some(&token))
        .await
        .expect_err("pre-cancelled call must not run");
    assert!(matches!(error, McpError::Cancelled));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "pre-cancelled call blocked for {:?}",
        started.elapsed()
    );
}
