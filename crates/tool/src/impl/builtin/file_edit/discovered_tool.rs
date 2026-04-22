use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::FileEditExecutor;
use super::spec::FileEditToolSpec;
use crate::r#impl::ToolRuntimeServices;

pub(crate) fn discover_file_edit(services: ToolRuntimeServices) -> DiscoveredTool {
    let spec = Arc::new(FileEditToolSpec::new());
    let executor = FileEditExecutor::new(Arc::clone(&spec), services);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
