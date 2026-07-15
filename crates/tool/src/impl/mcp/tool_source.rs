use agent_contracts::tool::{DiscoveredTool, ToolSource};
use mcp::McpServerWithTools;
use std::sync::Arc;

use super::executor::McpToolExecutor;
use super::spec::McpToolSpec;

/// Internal record: one MCP tool plus the client that owns it.
pub(super) struct McpDiscoveredTool {
    pub tool: mcp::McpToolDef,
    pub client: Arc<mcp::McpClient>,
    pub effect: mcp::EffectSection,
}

/// Tool source backed by pre-initialised MCP servers. Clients must be
/// connected, `initialize()`d, and `list_tools()`d before construction; the
/// resolver does this via `mcp::init_mcp_tools(...).await`.
pub struct McpToolSource {
    servers: Vec<McpServerWithTools>,
}

impl McpToolSource {
    pub fn new(servers: Vec<McpServerWithTools>) -> Self {
        Self { servers }
    }

    fn collect(&self) -> Vec<McpDiscoveredTool> {
        let mut out = Vec::new();
        for server in &self.servers {
            for tool in &server.tools {
                out.push(McpDiscoveredTool {
                    tool: tool.clone(),
                    client: Arc::clone(&server.client),
                    effect: server.effect.clone(),
                });
            }
        }
        out
    }
}

impl ToolSource for McpToolSource {
    fn discover(&self) -> Vec<DiscoveredTool> {
        self.collect()
            .into_iter()
            .map(|d| {
                let spec = McpToolSpec::new(&d);
                let executor =
                    McpToolExecutor::new(Arc::clone(&spec), d.client, d.tool.name.clone());
                DiscoveredTool {
                    spec: spec as Arc<dyn agent_contracts::tool::ToolSpecView>,
                    executor: Arc::new(executor),
                }
            })
            .collect()
    }
}
