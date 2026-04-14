use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolRegistry, ToolSpecView};
use agent_types::common::ids::{AgentId, ToolId};
use std::sync::Arc;

#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyToolRegistry;

impl EmptyToolRegistry {
    pub fn new() -> Self {
        Self
    }
}

impl ToolRegistry for EmptyToolRegistry {
    fn get_executor(&self, _id: &ToolId) -> Option<Arc<dyn ToolExecutor>> {
        None
    }

    fn get_spec(&self, _id: &ToolId) -> Option<&dyn ToolSpecView> {
        None
    }

    fn list_specs(&self) -> Vec<&dyn ToolSpecView> {
        Vec::new()
    }

    fn filter_for(&self, _agent_id: &AgentId) -> Box<dyn ToolFilter> {
        Box::new(EmptyToolFilter)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct EmptyToolFilter;

impl ToolFilter for EmptyToolFilter {
    fn visible_tools(&self) -> Vec<&dyn ToolSpecView> {
        Vec::new()
    }

    fn allows_tool_name(&self, _tool_name: &str) -> bool {
        false
    }

    fn get_spec_for_name(&self, _tool_name: &str) -> Option<Arc<dyn ToolSpecView>> {
        None
    }

    fn get_executor_for_name(&self, _tool_name: &str) -> Option<Arc<dyn ToolExecutor>> {
        None
    }
}
