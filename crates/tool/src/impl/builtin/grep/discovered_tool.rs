use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::GrepExecutor;
use super::spec::GrepToolSpec;

pub(crate) fn discover_grep() -> DiscoveredTool {
    let spec = Arc::new(GrepToolSpec::new());
    let executor = GrepExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
