use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::GlobToolExecutor;
use super::spec::GlobToolSpec;

pub(crate) fn discover_glob() -> DiscoveredTool {
    let spec = Arc::new(GlobToolSpec::new());
    let executor = GlobToolExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
