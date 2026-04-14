//! Plugin tool sources.

use agent_contracts::tool::{DiscoveredTool, ToolSource};

/// A plugin tool source.
pub struct PluginToolSource {}

impl PluginToolSource {
    /// Creates a new plugin tool source.
    pub fn new() -> Self {
        Self {}
    }
}

impl ToolSource for PluginToolSource {
    fn discover(&self) -> Vec<DiscoveredTool> {
        // TODO: Implement actual tool discovery when designed
        Vec::new()
    }
}
