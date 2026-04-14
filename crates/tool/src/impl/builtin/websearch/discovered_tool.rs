use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::WebSearchExecutor;
use super::spec::WebSearchToolSpec;

pub(crate) fn discover_web_search() -> DiscoveredTool {
    let spec = Arc::new(WebSearchToolSpec::new());
    let executor = WebSearchExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
