use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use lsp::LspServiceRegistry;
use subagent::SubagentControl;

#[derive(Clone, Default)]
pub struct SubagentRoleInfo {
    pub role_id: String,
    pub description: String,
}

#[derive(Clone, Default)]
pub struct ToolRuntimeServices {
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub workspace_root: Option<PathBuf>,
    pub subagent_roles: BTreeMap<String, SubagentRoleInfo>,
}
