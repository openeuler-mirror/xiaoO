use std::sync::Arc;

use lsp::LspServiceRegistry;
use subagent::SubagentControl;

#[derive(Clone, Default)]
pub struct ToolRuntimeServices {
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
    /// Registry of LSP services keyed by backend. Each tool invocation looks
    /// up (or lazily creates) the service that matches the session's backend.
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
}
