use crate::gateway::{
    AppTurnRequest, GatewayEntryContext, GatewayEntryKind, SessionOpenRequest, SessionRecord,
    SessionRuntimeBindings,
};
use agent_contracts::backend::OperationBackendConfig;
use agent_contracts::{CompressionPipeline, SkillRegistry, ToolRegistry};
use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::hook::HookerRegistryConfig;
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use subagent::SubagentControl;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeDescriptor {
    pub agent_id: AgentId,
    pub model: String,
    pub system_prompt: String,
    pub feature_flags: FeatureFlags,
    pub token_budget: TokenBudgetConfig,
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub max_turns: Option<u32>,
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
    pub operation_backend: Option<OperationBackendConfig>,
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
        }
    }
}

#[derive(Debug, Error)]
pub enum SessionRuntimeResolveError {
    #[error("runtime resolution failed: {message}")]
    ResolveFailed { message: String },
}

#[async_trait]
pub trait SessionRuntimeResolver: Send + Sync {
    fn bind_subagent_control(&self, _control: Arc<dyn SubagentControl>) {}

    async fn resolve(
        &self,
        request: &SessionRuntimeBuildInput,
        existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError>;
}
