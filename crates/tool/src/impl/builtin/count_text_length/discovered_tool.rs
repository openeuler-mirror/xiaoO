use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::CountTextLengthToolExecutor;
use super::spec::CountTextLengthToolSpec;

pub(crate) fn discover_count_text_length() -> DiscoveredTool {
    let spec = Arc::new(CountTextLengthToolSpec::new());
    let executor = CountTextLengthToolExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
