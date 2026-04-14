use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::FileWriteExecutor;
use super::spec::FileWriteToolSpec;

pub(crate) fn discover_file_write() -> DiscoveredTool {
    let spec = Arc::new(FileWriteToolSpec::new());
    let executor = FileWriteExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
