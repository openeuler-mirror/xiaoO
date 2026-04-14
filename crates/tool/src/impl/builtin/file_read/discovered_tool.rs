use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::FileReadExecutor;
use super::spec::FileReadToolSpec;

pub(crate) fn discover_file_read() -> DiscoveredTool {
    let spec = Arc::new(FileReadToolSpec::new());
    let executor = FileReadExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
