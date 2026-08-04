//! Verifies the tool-crate MCP glue: a connected MCP server's tools surface
//! through `McpToolSource::discover()` with the correct namespaced names, and
//! `McpToolExecutor::invoke()` calls back into the server.
//!
//! Requires `python3` on PATH — the test fails (not skips) when python3
//! is missing, so CI environments without it are surfaced explicitly
//! rather than reported as a green pass with zero assertions run.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use agent_contracts::tool::ToolSource;
use agent_types::tool::call_types::FinalToolCall;
use mcp::{McpServerConfig, Transport};
use tool::McpToolSource;

fn python_available() -> bool {
    Command::new("python3")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

struct NoopRuntime;

#[async_trait::async_trait]
impl agent_contracts::runtime::runtime_view::RuntimeView for NoopRuntime {
    fn state_store(&self) -> &dyn agent_contracts::tool::state::ToolStateStore {
        unreachable!("mcp executor does not touch state store")
    }
    fn tool_events(&self) -> &dyn agent_contracts::events::tool_events::ToolEventSink {
        unreachable!("mcp executor does not emit events directly")
    }
    fn trace_recorder(&self) -> &dyn agent_contracts::trace::TraceRecorder {
        unreachable!("mcp executor does not record traces directly")
    }
    fn agent_context(&self) -> &dyn agent_contracts::runtime::agent_context::AgentContext {
        unreachable!("mcp executor does not read agent context")
    }
    fn interaction(&self) -> &dyn agent_contracts::interaction::handle::InteractionHandle {
        unreachable!("mcp executor does not interact")
    }
    fn hookers(&self) -> &dyn agent_contracts::hook::registry::HookerRegistry {
        unreachable!("mcp executor does not consult hookers")
    }
}

#[tokio::test]
async fn mcp_tool_source_discovers_and_invokes() {
    if !python_available() {
        panic!("python3 not available on PATH; tool-crate mcp test requires python3 to run");
    }

    let script: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("mcp")
        .join("tests")
        .join("mock_server.py");

    let config = McpServerConfig {
        name: "mock".to_string(),
        transport: Transport::Stdio,
        command: Some("python3".to_string()),
        args: vec![script.to_string_lossy().into_owned()],
        env: HashMap::new(),
        url: None,
        bearer_token_env: None,
        agent_id: None,
        headers: std::collections::BTreeMap::new(),
        enabled: Some(true),
        timeout_ms: 10_000,
        effect: mcp::EffectSection::default(),
    };

    let servers = mcp::init_mcp_tools(std::slice::from_ref(&config)).await;
    assert_eq!(servers.len(), 1);

    let source = McpToolSource::new(servers);
    let discovered = source.discover();
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].spec.name().0, "mcp__mock__echo");
    assert_eq!(discovered[0].spec.id().0, "mcp__mock__echo");

    let call = FinalToolCall {
        call_id: "call-1".to_string(),
        tool_name: "mcp__mock__echo".to_string(),
        input: serde_json::json!({"message": "hi"}),
    };
    let runtime = NoopRuntime;
    let output = discovered[0]
        .executor
        .invoke(&call, &runtime)
        .await
        .expect("invoke echo");
    match output {
        agent_types::tool::execution_types::ToolExecutorOutput::Completed { raw_outcome } => {
            match raw_outcome {
                agent_types::tool::execution_types::RawToolOutcome::Success { output } => {
                    assert_eq!(output, "echo:hi");
                }
                other => panic!("expected success, got {other:?}"),
            }
        }
        other => panic!("expected completed, got {other:?}"),
    }

    // Prevent leaking the Arc references before the server child is killed.
    drop(discovered);
    drop(source);
}
