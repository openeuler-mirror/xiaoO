use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::WebFetchExecutor;
use super::spec::WebFetchToolSpec;

pub(crate) fn discover_webfetch() -> DiscoveredTool {
    let spec = Arc::new(WebFetchToolSpec::new());
    let executor = WebFetchExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
