use agent_contracts::context::budget::TokenBudgetPolicy;
use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolSpecView, ToolStateStoreBuilder};
use agent_contracts::trace::TraceRecorderBuilder;
use agent_contracts::{
    CompressionPipeline, InteractionHandle, PromptBuilder, SkillRegistry, ToolEventSink,
    ToolRegistry,
};
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::common::{AgentMetadata, BuildError};
use agent_types::events::ToolLifecycleEvent;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::tool::ToolStateStoreConfig;
use agent_types::tool::{EffectProfile, InputSchemaRef, OutputContract};
use async_trait::async_trait;
use compact::{CompactionPolicy, PassthroughCompressionPipeline};
use hook::framework::HookerRegistryBuilderImpl;
use hook::HookerRegistryBuilder;
use prompt::PromptBuilderImpl;
use serde_json::Value;
use std::sync::Arc;
use tool::{EmptyToolRegistry, ToolStateStoreBuilderImpl};
use trace::TraceRecorderBuilderImpl;
use xiaoo_core::{
    AgentRuntime, AgentRuntimeBuilder, BasicAgentContext, BasicRuntimeView, EmptySkillRegistry,
    NoopInteractionHandle, NoopToolEventSink,
};

use crate::gateway::{GatewayEntryKind, ResolvedSessionRuntime, SessionRecord};
use xiaoo_core::LoopStateSnapshot;

pub struct AppRuntimeAssembly {
    pub runtime: AgentRuntime,
    pub runtime_view: Option<Arc<dyn RuntimeView>>,
    pub visible_tools: Vec<Arc<dyn ToolSpecView>>,
}

pub struct AppRuntimeFactory;

#[derive(Debug, thiserror::Error)]
pub enum AppRuntimeFactoryError {
    #[error("core runtime build failed: {0}")]
    CoreBuild(#[from] BuildError),
    #[error("trace config serialization failed: {0}")]
    TraceConfigSerialization(#[from] serde_json::Error),
}

impl AppRuntimeFactory {
    pub async fn build(
        resolved: &ResolvedSessionRuntime,
        session: &SessionRecord,
        loop_state: Option<&LoopStateSnapshot>,
    ) -> Result<AppRuntimeAssembly, AppRuntimeFactoryError> {
        let prompt_builder: Arc<dyn PromptBuilder> = Arc::new(PromptBuilderImpl::new());
        let compression_pipeline: Arc<dyn CompressionPipeline> = resolved
            .compression_pipeline
            .clone()
            .unwrap_or_else(|| Arc::new(PassthroughCompressionPipeline::new()));
        let tool_registry: Arc<dyn ToolRegistry> = resolved
            .tool_registry
            .clone()
            .unwrap_or_else(|| Arc::new(EmptyToolRegistry::new()));
        let skill_registry: Arc<dyn SkillRegistry> = resolved
            .skill_registry
            .clone()
            .unwrap_or_else(|| Arc::new(EmptySkillRegistry::new()));
        let token_budget_policy: Arc<dyn TokenBudgetPolicy> = Arc::new(
            CompactionPolicy::from_budget(&resolved.descriptor.token_budget),
        );
        let is_channel_session = session.channel.is_some();
        let visible_tools = tool_registry
            .filter_for(&resolved.descriptor.agent_id)
            .visible_tools()
            .into_iter()
            .filter(|spec| {
                // Hide channel-only tools when not in a channel session.
                if !is_channel_session {
                    let name = spec.name().0.as_str();
                    if CHANNEL_ONLY_TOOLS.contains(&name) {
                        return false;
                    }
                }
                true
            })
            .map(|spec| Arc::new(DetachedToolSpec::from(spec)) as Arc<dyn ToolSpecView>)
            .collect::<Vec<_>>();

        let runtime_view = {
            let hookers = HookerRegistryBuilderImpl::new()
                .with_config(resolved.hooker.clone())
                .build()?;
            let agent_context = BasicAgentContext::new(
                loop_state
                    .map(|snapshot| snapshot.messages.clone())
                    .unwrap_or_default(),
                resolved.descriptor.workspace_root.clone(),
                AgentMetadata {
                    agent_id: resolved.descriptor.agent_id.0.clone(),
                    model: resolved.descriptor.model.clone(),
                    session_id: Some(session.session_id.clone()),
                },
            );
            let mut trace_config = resolved.trace.clone();
            let trace_config_obj = trace_config.as_object_mut().ok_or_else(|| {
                serde_json::Error::io(std::io::Error::other(
                    "trace config must serialize to a JSON object",
                ))
            })?;
            trace_config_obj.insert(
                "agent_id".to_string(),
                Value::String(resolved.descriptor.agent_id.0.clone()),
            );
            trace_config_obj.insert(
                "workspace_root".to_string(),
                Value::String(resolved.descriptor.workspace_root.display().to_string()),
            );
            let trace_recorder = TraceRecorderBuilderImpl::default()
                .from_json(trace_config)?
                .build()
                .await?;
            let inner = BasicRuntimeView::new(
                ToolStateStoreBuilderImpl::new()
                    .with_config(tool_state_store_config_for_entry_kind(
                        resolved.entry_kind.as_ref(),
                    ))
                    .build()?,
                Box::new(SharedToolEventSink::new(
                    resolved.bindings.tool_event_sink.clone(),
                )),
                trace_recorder,
                Box::new(agent_context),
                Box::new(SharedInteractionHandle::new(
                    resolved.bindings.interaction_handle.clone(),
                )),
                hookers,
            );
            let runtime_view: Arc<dyn RuntimeView> = Arc::new(SkillAwareRuntimeView {
                inner,
                skill_registry: skill_registry.clone(),
                channel_file_sender: resolved.bindings.channel_file_sender.clone(),
            });

            Some(runtime_view)
        };

        if let Some(rv) = &runtime_view {
            resolved.llm_provider.set_runtime_view(rv.clone());
        }

        let mut builder = AgentRuntimeBuilder::new()
            .llm_provider(Arc::clone(&resolved.llm_provider))
            .compression_pipeline(compression_pipeline)
            .prompt_builder(prompt_builder)
            .system_prompt(resolved.descriptor.system_prompt.clone())
            .tool_registry(tool_registry)
            .skill_registry(skill_registry)
            .feature_flags(resolved.descriptor.feature_flags.clone())
            .token_budget_config(resolved.descriptor.token_budget.clone())
            .token_budget_policy(token_budget_policy);

        if let Some(max_turns) = resolved.descriptor.max_turns {
            builder = builder.max_turns(max_turns);
        }

        let runtime = builder.build()?;

        Ok(AppRuntimeAssembly {
            runtime,
            runtime_view,
            visible_tools,
        })
    }
}

fn tool_state_store_config_for_entry_kind(
    entry_kind: Option<&GatewayEntryKind>,
) -> ToolStateStoreConfig {
    let backend = match entry_kind {
        Some(GatewayEntryKind::Tui | GatewayEntryKind::Cli) => "noop",
        _ => "stdout",
    };

    ToolStateStoreConfig {
        backend: Value::String(backend.to_string()),
        retention: Value::Null,
    }
}

#[derive(Clone)]
struct DetachedToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl From<&dyn ToolSpecView> for DetachedToolSpec {
    fn from(value: &dyn ToolSpecView) -> Self {
        Self {
            id: value.id().clone(),
            name: value.name().clone(),
            description: value.description().to_string(),
            input_schema: value.input_schema().clone(),
            output_contract: value.output_contract().clone(),
            effect_profile: value.effect_profile().clone(),
        }
    }
}

impl ToolSpecView for DetachedToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}

struct SharedToolEventSink {
    inner: Option<Arc<dyn ToolEventSink>>,
}

impl SharedToolEventSink {
    fn new(inner: Option<Arc<dyn ToolEventSink>>) -> Self {
        Self { inner }
    }
}

impl ToolEventSink for SharedToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        if let Some(inner) = &self.inner {
            inner.emit(event);
            return;
        }
        NoopToolEventSink::new().emit(event);
    }
}

struct SharedInteractionHandle {
    inner: Option<Arc<dyn InteractionHandle>>,
}

impl SharedInteractionHandle {
    fn new(inner: Option<Arc<dyn InteractionHandle>>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl InteractionHandle for SharedInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        if let Some(inner) = &self.inner {
            return inner.ask(request).await;
        }
        NoopInteractionHandle::new().ask(request).await
    }
}

// ---------------------------------------------------------------------------
// SkillAwareRuntimeView — delegates to BasicRuntimeView, overrides skill_registry()
// ---------------------------------------------------------------------------

/// Tools that should only be visible in channel sessions (e.g. Feishu).
const CHANNEL_ONLY_TOOLS: &[&str] = &["send_file"];

struct SkillAwareRuntimeView {
    inner: BasicRuntimeView,
    skill_registry: Arc<dyn SkillRegistry>,
    channel_file_sender: Option<Arc<dyn agent_contracts::ChannelFileSender>>,
}

impl RuntimeView for SkillAwareRuntimeView {
    fn state_store(&self) -> &dyn agent_contracts::ToolStateStore {
        self.inner.state_store()
    }
    fn tool_events(&self) -> &dyn agent_contracts::ToolEventSink {
        self.inner.tool_events()
    }
    fn trace_recorder(&self) -> &dyn agent_contracts::TraceRecorder {
        self.inner.trace_recorder()
    }
    fn agent_context(&self) -> &dyn agent_contracts::AgentContext {
        self.inner.agent_context()
    }
    fn interaction(&self) -> &dyn agent_contracts::InteractionHandle {
        self.inner.interaction()
    }
    fn hookers(&self) -> &dyn agent_contracts::HookerRegistry {
        self.inner.hookers()
    }
    fn skill_registry(&self) -> Option<&dyn SkillRegistry> {
        Some(self.skill_registry.as_ref())
    }
    fn channel_file_sender(&self) -> Option<&dyn agent_contracts::ChannelFileSender> {
        self.channel_file_sender.as_deref()
    }
}
