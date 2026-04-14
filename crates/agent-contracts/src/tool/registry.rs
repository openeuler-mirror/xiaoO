use crate::tool::executor::ToolExecutor;
use crate::tool::spec::ToolSpecView;
use agent_types::common::ids::{AgentId, ToolId};
use std::sync::Arc;

pub trait ToolRegistry: Send + Sync {
    fn get_executor(&self, id: &ToolId) -> Option<Arc<dyn ToolExecutor>>;
    fn get_spec(&self, id: &ToolId) -> Option<&dyn ToolSpecView>;
    fn list_specs(&self) -> Vec<&dyn ToolSpecView>;
    fn filter_for(&self, agent_id: &AgentId) -> Box<dyn ToolFilter>;
}

pub trait ToolFilter: Send + Sync {
    fn visible_tools(&self) -> Vec<&dyn ToolSpecView>;

    fn allows_tool_name(&self, tool_name: &str) -> bool;

    fn get_spec_for_name(&self, tool_name: &str) -> Option<Arc<dyn ToolSpecView>>;

    fn get_executor_for_name(&self, tool_name: &str) -> Option<Arc<dyn ToolExecutor>>;
}
