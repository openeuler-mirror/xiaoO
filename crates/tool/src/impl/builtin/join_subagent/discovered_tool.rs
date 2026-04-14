use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use crate::r#impl::ToolRuntimeServices;

use super::executor::JoinSubagentExecutor;
use super::spec::JoinSubagentToolSpec;

pub(crate) fn discover_join_subagent(services: ToolRuntimeServices) -> DiscoveredTool {
    let spec = Arc::new(JoinSubagentToolSpec::new());
    let executor = JoinSubagentExecutor::new(Arc::clone(&spec), services);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
