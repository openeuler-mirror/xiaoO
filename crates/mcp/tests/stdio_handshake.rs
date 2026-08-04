//! End-to-end test: spawn the python mock MCP server, run the full
//! connect → initialize → list_tools → call_tool flow, and verify the
//! surfaced tool can be invoked through the `McpToolSource` plumbing.
//!
//! Requires `python3` on PATH — the test fails (not skips) when python3
//! is missing, so CI environments without it are surfaced explicitly
//! rather than reported as a green pass with zero assertions run.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use mcp::{McpServerConfig, Transport};

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn mock_server_config(script_path: PathBuf) -> McpServerConfig {
    McpServerConfig {
        name: "mock".to_string(),
        transport: Transport::Stdio,
        command: Some("python3".to_string()),
        args: vec![script_path.to_string_lossy().into_owned()],
        env: HashMap::new(),
        url: None,
        bearer_token_env: None,
        agent_id: None,
        headers: std::collections::BTreeMap::new(),
        enabled: Some(true),
        timeout_ms: 10_000,
        effect: mcp::EffectSection::default(),
    }
}

#[tokio::test]
async fn stdio_initialize_list_and_call() {
    if !python_available() {
        panic!("python3 not available on PATH; mcp stdio test requires python3 to run");
    }

    let script: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("mock_server.py");

    let config = mock_server_config(script);
    let client = mcp::McpClient::connect(&config)
        .await
        .expect("connect mock server");
    let init = client.initialize().await.expect("initialize");
    assert_eq!(init.server_info.unwrap().name, "mock");

    let tools = client.list_tools().await.expect("list_tools");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[0].description, "Echo the provided message");

    let result = client
        .call_tool("echo", serde_json::json!({"message": "hello"}))
        .await
        .expect("call_tool");
    assert!(!result.is_error);
    assert_eq!(result.flatten_text(), "echo:hello");

    // Unknown tool should surface a server error.
    let err = client
        .call_tool("nope", serde_json::json!({}))
        .await
        .expect_err("unknown tool must error");
    assert!(matches!(err, mcp::McpError::ServerError { .. }));
}

#[tokio::test]
async fn init_mcp_tools_surfaces_mock_server() {
    if !python_available() {
        panic!("python3 not available on PATH; mcp tool-listing test requires python3 to run");
    }

    let script: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("mock_server.py");
    let config = mock_server_config(script);

    let servers = mcp::init_mcp_tools(std::slice::from_ref(&config)).await;
    assert_eq!(servers.len(), 1, "exactly one server should come up");
    assert_eq!(servers[0].client.server_name(), "mock");
    assert_eq!(servers[0].tools.len(), 1);
    assert_eq!(servers[0].tools[0].name, "echo");
}
