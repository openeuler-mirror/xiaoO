use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::FileEditExecutor;
use super::spec::FileEditToolSpec;

pub(crate) fn discover_file_edit() -> DiscoveredTool {
    let spec = Arc::new(FileEditToolSpec::new());
    let executor = FileEditExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
