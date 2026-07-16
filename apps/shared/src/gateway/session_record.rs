use crate::backend::BackendCheckpointRef;
use crate::gateway::{GatewayEntryContext, LlmRuntimeConfig};
use agent_contracts::backend::BackendInstance;
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
    Paused,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubagentRoleRecord {
    pub role_id: String,
    pub description: String,
    pub prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub tools: BTreeMap<String, bool>,
}

pub const E2B_BOOTSTRAP_MANIFEST_VERSION: u32 = 1;
pub const E2B_REMOTE_WORKSPACE_ROOT: &str = "/home/user/workspace";
pub const E2B_REMOTE_SKILLS_ROOT: &str = "/home/user/.xiaoo/skills";

/// Immutable host-to-E2B bootstrap identity persisted with the runtime.
/// Host paths are only consulted while the first sandbox is being created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBootstrapBinding {
    #[serde(default)]
    pub source_workspace: Option<PathBuf>,
    #[serde(default)]
    pub source_skill_roots: Vec<PathBuf>,
    pub content_digest: String,
    pub remote_workspace_root: PathBuf,
    #[serde(default)]
    pub remote_skill_roots: Vec<PathBuf>,
    #[serde(default)]
    pub skills: Vec<RuntimeBootstrapSkill>,
    pub manifest_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeBootstrapSkill {
    pub name: String,
    pub remote_dir: PathBuf,
    pub manifest_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeSnapshot {
    pub agent_id: AgentId,
    pub model: String,
    #[serde(default)]
    pub llm: Option<LlmRuntimeConfig>,
    pub system_prompt: String,
    pub feature_flags: FeatureFlags,
    pub token_budget: TokenBudgetConfig,
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub max_turns: Option<u32>,
    #[serde(default)]
    pub tool_manifest: Option<Vec<ToolSpecSnapshot>>,
    #[serde(default)]
    pub subagent_roles: BTreeMap<String, SubagentRoleRecord>,
    #[serde(default)]
    pub bootstrap_binding: Option<RuntimeBootstrapBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    #[serde(rename = "runtime_id", alias = "session_id")]
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
    #[serde(default)]
    pub backend_instance: Option<BackendInstance>,
    /// Checkpoint captured when the session's sandbox was evicted to free a
    /// sandbox slot. When set, the session is paused and can be resumed by
    /// checking out a new sandbox from this checkpoint on the next turn.
    #[serde(default)]
    pub paused_backend_checkpoint: Option<BackendCheckpointRef>,
    pub loop_state: Option<LoopStateSnapshot>,
    pub memory_snapshot: Option<MemorySnapshot>,
    #[serde(default)]
    pub agents: BTreeMap<String, SessionAgentRecord>,
    #[serde(default)]
    pub subagent_state: SubagentSessionState,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Runtime that this runtime was forked from via `checkout`. `None` for
    /// sessions created directly via `open`. Used by `force_close_session` to
    /// cascade-close descendants.
    #[serde(default)]
    pub parent_runtime_id: Option<String>,
    /// Checkpoint from which this runtime was forked via `checkout`. `None`
    /// for sessions created directly via `open`.
    #[serde(default)]
    pub forked_from_checkpoint_id: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionAgentRecord {
    pub agent_id: AgentId,
    #[serde(default)]
    pub parent_agent_id: Option<AgentId>,
    #[serde(default)]
    pub subagent_role_id: Option<String>,
    pub loop_state: Option<LoopStateSnapshot>,
    pub memory_snapshot: Option<MemorySnapshot>,
    #[serde(default)]
    pub tool_manifest: Option<Vec<ToolSpecSnapshot>>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}
