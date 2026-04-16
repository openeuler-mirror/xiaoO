use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::SendFileToolExecutor;
use super::spec::SendFileToolSpec;

pub(crate) fn discover_send_file() -> DiscoveredTool {
    let spec = Arc::new(SendFileToolSpec::new());
    let executor = SendFileToolExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
