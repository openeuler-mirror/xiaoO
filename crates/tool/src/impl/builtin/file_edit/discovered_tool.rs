use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;
use tokio::sync::Mutex;

use super::executor::FileEditExecutor;
use super::spec::FileEditToolSpec;
use crate::r#impl::builtin::file_read::DedupStateStore;
use crate::r#impl::ToolRuntimeServices;

pub(crate) fn discover_file_edit(
    services: ToolRuntimeServices,
    file_read_state: Arc<Mutex<DedupStateStore>>,
) -> DiscoveredTool {
    let spec = Arc::new(FileEditToolSpec::new());
    let executor = FileEditExecutor::new_with_state(Arc::clone(&spec), services, file_read_state);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
