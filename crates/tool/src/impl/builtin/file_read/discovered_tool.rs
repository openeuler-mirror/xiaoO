use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::FileReadExecutor;
use super::spec::FileReadToolSpec;
use crate::r#impl::ToolRuntimeServices;

pub(crate) fn discover_file_read(services: ToolRuntimeServices) -> DiscoveredTool {
    let spec = Arc::new(FileReadToolSpec::new());
    let executor = FileReadExecutor::new(Arc::clone(&spec), services);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
