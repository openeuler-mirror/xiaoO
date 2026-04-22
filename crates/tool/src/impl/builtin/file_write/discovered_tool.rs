use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::FileWriteExecutor;
use super::spec::FileWriteToolSpec;
use crate::r#impl::ToolRuntimeServices;

pub(crate) fn discover_file_write(services: ToolRuntimeServices) -> DiscoveredTool {
    let spec = Arc::new(FileWriteToolSpec::new());
    let executor = FileWriteExecutor::new(Arc::clone(&spec), services);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
