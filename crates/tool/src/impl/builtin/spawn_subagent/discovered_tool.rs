use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use crate::r#impl::ToolRuntimeServices;

use super::executor::SpawnSubagentExecutor;
use super::spec::SpawnSubagentToolSpec;

pub(crate) fn discover_spawn_subagent(services: ToolRuntimeServices) -> DiscoveredTool {
    let spec = Arc::new(SpawnSubagentToolSpec::with_subagent_roles(
        services.subagent_roles.clone(),
    ));
    let executor = SpawnSubagentExecutor::new(Arc::clone(&spec), services);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
