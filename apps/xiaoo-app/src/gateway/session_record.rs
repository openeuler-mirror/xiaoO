use crate::gateway::GatewayEntryContext;
use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use memory::MemorySnapshot;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use subagent::SubagentSessionState;
use tool::ToolSpecSnapshot;
use xiaoo_core::LoopStateSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionLifecycleStatus {
    Idle,
    Running,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeSnapshot {
    pub agent_id: AgentId,
    pub model: String,
    pub system_prompt: String,
    pub feature_flags: FeatureFlags,
    pub token_budget: TokenBudgetConfig,
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tool_manifest: Option<Vec<ToolSpecSnapshot>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    pub session_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    #[serde(default)]
    pub entry: GatewayEntryContext,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    pub status: SessionLifecycleStatus,
    pub runtime: SessionRuntimeSnapshot,
    pub loop_state: Option<LoopStateSnapshot>,
    pub memory_snapshot: Option<MemorySnapshot>,
    #[serde(default)]
    pub agents: BTreeMap<String, SessionAgentRecord>,
    #[serde(default)]
    pub subagent_state: SubagentSessionState,
    pub last_error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    pub agent_id: AgentId,
    #[serde(default)]
    pub parent_agent_id: Option<AgentId>,
    pub loop_state: Option<LoopStateSnapshot>,
    pub memory_snapshot: Option<MemorySnapshot>,
    #[serde(default)]
    pub tool_manifest: Option<Vec<ToolSpecSnapshot>>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
