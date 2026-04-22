use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use crate::r#impl::ToolRuntimeServices;

use super::executor::LspExecutor;
use super::spec::LspToolSpec;

pub(crate) fn discover_lsp(services: ToolRuntimeServices) -> DiscoveredTool {
    let spec = Arc::new(LspToolSpec::new());
    let executor = LspExecutor::new(Arc::clone(&spec), services);
    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
