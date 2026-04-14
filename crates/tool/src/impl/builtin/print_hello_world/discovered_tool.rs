use std::sync::Arc;

use agent_contracts::tool::DiscoveredTool;

use super::executor::PrintHelloWorldToolExecutor;
use super::spec::PrintHelloWorldToolSpec;

pub(crate) fn discover_print_hello_world() -> DiscoveredTool {
    let spec = Arc::new(PrintHelloWorldToolSpec::new());
    let executor = PrintHelloWorldToolExecutor::new(Arc::clone(&spec));

    DiscoveredTool {
        spec,
        executor: Arc::new(executor),
    }
}
