use std::sync::Arc;

use agent_contracts::lsp::LspProvider;
use subagent::SubagentControl;

#[derive(Clone, Default)]
pub struct ToolRuntimeServices {
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
    pub lsp_service: Option<Arc<dyn LspProvider>>,
}
