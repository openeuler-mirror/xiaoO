use crate::backend::GatewayBackendConfig;
use crate::gateway::session_record::SubagentRoleRecord;
use crate::gateway::tool_assembly::BoundControlStore;
use crate::gateway::{
    AppTurnRequest, GatewayEntryContext, GatewayEntryKind, LlmRuntimeConfig, SessionOpenRequest,
    SessionRecord,
};
use agent_contracts::{CompressionPipeline, SkillRegistry, ToolRegistry};
use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::HookerRegistryConfig;
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use subagent::SubagentControl;
use thiserror::Error;

use super::SessionRuntimeBindings;
use crate::gateway::RuntimeBootstrapBinding;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeDescriptor {
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
    pub subagent_roles: BTreeMap<String, SubagentRoleRecord>,
}

pub struct ResolvedSessionRuntime {
    pub descriptor: SessionRuntimeDescriptor,
    pub entry_kind: Option<GatewayEntryKind>,
    pub llm_provider: Arc<LlmProviderWrapper>,
    pub tool_registry: Option<Arc<dyn ToolRegistry>>,
    pub skill_registry: Option<Arc<dyn SkillRegistry>>,
    pub bindings: SessionRuntimeBindings,
    pub compression_pipeline: Option<Arc<dyn CompressionPipeline>>,
    pub trace: Value,
    pub hooker: HookerRegistryConfig,
    pub operation_backend: Option<GatewayBackendConfig>,
    /// Host path used only as the backend/session identity. For E2B this is
    /// deliberately separate from `descriptor.workspace_root`, which is remote.
    pub backend_workspace_root: PathBuf,
    /// A first-creation E2B bootstrap archive. Existing/resumed runtimes leave this empty.
    pub e2b_bootstrap: Option<Arc<crate::backend::E2bBootstrapArchive>>,
    pub bootstrap_binding: Option<RuntimeBootstrapBinding>,
    pub e2b_finalized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeBuildInput {
    pub session_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    pub channel: Option<String>,
    pub channel_instance_id: Option<String>,
    pub channel_identity_prompt: Option<String>,
    pub entry: GatewayEntryContext,
    pub agent_id_override: Option<AgentId>,
    pub max_turns_override: Option<u32>,
    #[serde(default)]
    pub subagent_role_id: Option<String>,
    pub llm: Option<LlmRuntimeConfig>,
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    #[serde(default)]
    pub skills: Option<Vec<PathBuf>>,
}

impl SessionRuntimeBuildInput {
    pub fn from_turn_request(request: &AppTurnRequest) -> Self {
        Self {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            channel_identity_prompt: request.channel_identity_prompt.clone(),
            entry: request.entry.clone(),
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: request.llm.clone(),
            workspace: request.workspace.clone(),
            skills: request.skills.clone(),
        }
    }

    pub fn from_open_request(request: &SessionOpenRequest) -> Self {
        Self {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            channel_identity_prompt: None,
            entry: request.entry.clone(),
            agent_id_override: None,
            max_turns_override: None,
            subagent_role_id: None,
            llm: request.llm.clone(),
            workspace: request.workspace.clone(),
            skills: request.skills.clone(),
        }
    }
}

#[derive(Debug, Error)]
pub enum SessionRuntimeResolveError {
    #[error("runtime resolution failed: {message}")]
    ResolveFailed { message: String },
    #[error("invalid runtime bootstrap request: {message}")]
    InvalidBootstrap { message: String },
    #[error("runtime bootstrap binding conflict: {message}")]
    BootstrapConflict { message: String },
    #[error("runtime bootstrap payload exceeds capacity: {message}")]
    BootstrapTooLarge { message: String },
}

#[async_trait]
pub trait SessionRuntimeResolver: Send + Sync {
    fn bind_subagent_control(&self, _control: Arc<dyn SubagentControl>) {}

    /// 暴露一个不透明的已绑定控制句柄存储，供 `AppBootstrap` 启动期注入
    /// 绑定（与 [`SessionRuntimeResolver::bind_subagent_control`] 二选一：
    /// 解析器自行管理绑定时返回 `None`，否则返回共享 store 让 bootstrap 写入）。
    fn bound_control_store(&self) -> Option<&BoundControlStore> {
        None
    }

    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError>;
}
