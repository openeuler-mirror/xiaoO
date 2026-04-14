use crate::common::{AgentId, ToolName};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Boot-time configuration payload for constructing a tool registry.
///
/// This is a pure data carrier used by registry builders; it does not encode
/// any registry behavior by itself.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolRegistryConfig {
    /// Visibility policy applied when deriving an agent-scoped tool filter.
    pub visibility: ToolVisibilityConfig,
}

/// Declares which tools are visible to each agent at boot-time.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolVisibilityConfig {
    /// Mapping from agent id to the set of tool names that agent is allowed to see.
    pub per_agent_allowed_tools: HashMap<AgentId, Vec<ToolName>>,
}

/// Boot-time configuration payload for constructing a tool state store.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ToolStateStoreConfig {
    /// Host-selected backend configuration for persistence.
    pub backend: serde_json::Value,
    /// Host-selected retention policy for lifecycle records and related state.
    pub retention: serde_json::Value,
}
