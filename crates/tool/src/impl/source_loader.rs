//! Tool source loader.

use agent_contracts::tool::ToolSource;

use super::builtin::BuiltinToolSource;
use super::plugin::PluginToolSource;
use super::ToolRuntimeServices;

/// Loads all available tool sources.
///
/// Returns a collection of tool sources combining built-in and plugin sources.
pub fn load_tool_sources() -> Vec<Box<dyn ToolSource>> {
    load_tool_sources_with_services(ToolRuntimeServices::default())
}

pub fn load_tool_sources_with_services(services: ToolRuntimeServices) -> Vec<Box<dyn ToolSource>> {
    vec![
        Box::new(BuiltinToolSource::new(services)),
        Box::new(PluginToolSource::new()),
    ]
}
