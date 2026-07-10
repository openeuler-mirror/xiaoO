use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;
use tokio::sync::Mutex;

use super::dedup::DedupStateStore;
use super::executor::FileReadExecutor;
use super::spec::FileReadToolSpec;
use crate::r#impl::ToolRuntimeServices;

pub(crate) fn discover_file_read(
    services: ToolRuntimeServices,
    file_read_state: Arc<Mutex<DedupStateStore>>,
) -> DiscoveredTool {
    let spec = Arc::new(FileReadToolSpec::new());
    let executor = FileReadExecutor::new_with_state(Arc::clone(&spec), services, file_read_state);

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
