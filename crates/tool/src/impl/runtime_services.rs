use std::sync::Arc;

use subagent::SubagentControl;

#[derive(Clone, Default)]
pub struct ToolRuntimeServices {
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
}
