use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkspaceRef {
    pub root: PathBuf,
}

/// Render a workspace root path as the optional string carried by hook
/// payloads: `None` when the path is empty (no workspace bound), otherwise
/// the lossy string form of the root. This is the single definition of the
/// "empty root means no workspace" rule — every hook payload builder
/// (plugin tool/chat payloads, gateway session lifecycle hook inputs) must
/// derive its workspace field through it so the null/None semantics cannot
/// drift between dispatch paths.
pub fn workspace_root_string(root: &Path) -> Option<String> {
    if root.as_os_str().is_empty() {
        None
    } else {
        Some(root.to_string_lossy().into_owned())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentMetadata {
    pub agent_id: String,
    pub model: String,
    pub session_id: Option<String>,
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-types/common/agent_context_test.rs"]
mod tests;
