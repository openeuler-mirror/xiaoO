use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::TodoWriteToolExecutor;
use super::spec::TodoWriteToolSpec;

pub(crate) fn discover_todo_write() -> DiscoveredTool {
    let spec = Arc::new(TodoWriteToolSpec::new());
    let executor = TodoWriteToolExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
