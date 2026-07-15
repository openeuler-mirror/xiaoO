//! Tool source loader.

use agent_contracts::tool::ToolSource;

use super::builtin::BuiltinToolSource;
use super::mcp::McpToolSource;
use super::plugin::PluginToolSource;
use super::ToolRuntimeServices;

/// Loads all available tool sources.
///
/// Returns a collection of tool sources combining built-in and plugin sources.
pub fn load_tool_sources() -> Vec<Box<dyn ToolSource>> {
    load_tool_sources_with_services(ToolRuntimeServices::default())
}

pub fn load_tool_sources_with_services(
    mut services: ToolRuntimeServices,
) -> Vec<Box<dyn ToolSource>> {
    let workspace_root = services.workspace_root.clone();
    let mcp_servers = services.mcp_servers.take().unwrap_or_default();
    let mut sources: Vec<Box<dyn ToolSource>> = vec![
        Box::new(BuiltinToolSource::new(services)),
        Box::new(PluginToolSource::new(workspace_root)),
    ];
    if !mcp_servers.is_empty() {
        sources.push(Box::new(McpToolSource::new(mcp_servers)));
    }
    sources
}
