use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use lsp::LspServiceRegistry;
use subagent::SubagentControl;

use mcp::McpServerWithTools;

#[derive(Clone, Default)]
pub struct SubagentRoleConfig {
    pub description: String,
    pub prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub tools: BTreeMap<String, bool>,
}

#[derive(Clone, Default)]
pub struct ToolRuntimeServices {
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub workspace_root: Option<PathBuf>,
    pub subagent_roles: BTreeMap<String, SubagentRoleConfig>,
    /// Pre-initialised MCP servers with their listed tools. Populated by the
    /// runtime resolver before tool sources are loaded. `None` means "not yet
    /// initialised"; `Some(vec)` means initialisation has completed (even if
    /// the vec is empty, e.g. all servers were unreachable). This distinction
    /// prevents re-running the (expensive) init on every `resolve()` call when
    /// no servers are reachable.
    pub mcp_servers: Option<Vec<McpServerWithTools>>,
}
