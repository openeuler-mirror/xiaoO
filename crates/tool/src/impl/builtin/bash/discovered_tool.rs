use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::BashExecutor;
use super::spec::BashToolSpec;

pub(crate) fn discover_bash() -> DiscoveredTool {
    let spec = Arc::new(BashToolSpec::new());
    let executor = BashExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
