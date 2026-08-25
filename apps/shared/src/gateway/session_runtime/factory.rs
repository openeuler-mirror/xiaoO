use agent_contracts::backend::OperationBackend;
use agent_contracts::context::budget::TokenBudgetPolicy;
use agent_contracts::tool::{ToolSpecView, ToolStateStoreBuilder};
use agent_contracts::trace::TraceRecorderBuilder;
use agent_contracts::{
    CompressionPipeline, InteractionHandle, PromptBuilder, SkillRegistry, ToolEventSink,
    ToolRegistry,
};
use agent_types::common::{AgentId, AgentMetadata, BuildError};
use agent_types::events::ToolLifecycleEvent;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::tool::ToolStateStoreConfig;
use async_trait::async_trait;
use compact::{build_context_manager, CompactError, CompactionPolicy};
use hook::framework::HookerRegistryBuilderImpl;
use hook::HookerRegistryBuilder;
use prompt::PromptBuilderImpl;
use serde_json::Value;
use std::sync::Arc;
use tool::{
    snapshot_tool_specs, tool_specs_from_snapshot, EmptyToolRegistry, ToolSpecSnapshot,
    ToolStateStoreBuilderImpl,
};
use trace::TraceRecorderBuilderImpl;
use xiaoo_api::events::NoopToolEventSink;
use xiaoo_api::interaction::NoopInteractionHandle;
use xiaoo_api::runtime::{
    BasicAgentContext, BasicRuntimeView, Runtime, RuntimeBuildError, RuntimeView,
};
use xiaoo_api::skills::EmptySkillRegistry;

use parking_lot::RwLock;

use super::ResolvedSessionRuntime;
use crate::gateway::permission_backend::PermissionAwareOperationBackend;
use crate::gateway::{GatewayEntryKind, SessionRecord};

pub(crate) struct AppRuntimeAssembly {
    pub runtime: Runtime,
    pub runtime_view: Option<Arc<dyn RuntimeView>>,
    pub visible_tools: Vec<Arc<dyn ToolSpecView>>,
    pub tool_manifest: Vec<ToolSpecSnapshot>,
}

impl AppRuntimeAssembly {
    pub async fn shutdown(self) -> Result<(), agent_contracts::backend::OperationError> {
        let AppRuntimeAssembly {
            runtime,
            runtime_view,
            visible_tools: _,
            tool_manifest: _,
        } = self;

        drop(runtime_view);
        drop(runtime);

        Ok(())
    }
}

pub struct AppRuntimeFactory;

#[derive(Debug, thiserror::Error)]
pub(crate) enum AppRuntimeFactoryError {
    #[error("core runtime build failed: {0}")]
    CoreBuild(#[from] BuildError),
    #[error("runtime build failed: {0}")]
    ApiBuild(#[from] RuntimeBuildError),
    #[error("trace config serialization failed: {0}")]
    TraceConfigSerialization(#[from] serde_json::Error),
    #[error("compression pipeline build failed: {0}")]
    CompactBuild(#[from] CompactError),
}

impl AppRuntimeFactory {
    pub(crate) async fn build(
        resolved: &ResolvedSessionRuntime,
        session: &SessionRecord,
        messages: Arc<RwLock<Vec<agent_types::ChatMessage>>>,
        existing_tool_manifest: Option<Vec<ToolSpecSnapshot>>,
        operation_backend: Arc<dyn OperationBackend>,
    ) -> Result<AppRuntimeAssembly, AppRuntimeFactoryError> {
        let prompt_builder: Arc<dyn PromptBuilder> = Arc::new(PromptBuilderImpl::new());
        // Defense-in-depth: every code path that resolves a session runtime
        // should have already injected a real `ContextManager` (daemon:
        // `build_compression_pipeline`, local CLI: ditto). If a caller forgets
        // to set `compression_pipeline`, fall back to a real `ContextManager`
        // built from the shared defaults instead of a silent no-op
        // `PassthroughCompressionPipeline` — the no-op trap was what caused
        // the daemon to emit `Pre-check failed` / `context compression
        // triggered` with `removed=0` forever when `[compact]` was missing.
        let compression_pipeline: Arc<dyn CompressionPipeline> =
            match resolved.compression_pipeline.clone() {
                Some(pipeline) => pipeline,
                None => build_context_manager(None, Arc::clone(&resolved.llm_provider))?,
            };
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
        let tool_manifest = existing_tool_manifest.unwrap_or_else(|| {
            snapshot_tool_specs(
                tool_registry
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
                    }),
            )
        });
        let visible_tools = tool_specs_from_snapshot(&tool_manifest);

        let runtime_view = {
            let hookers = HookerRegistryBuilderImpl::new()
                .with_config(resolved.hooker.clone())
                .build()?;
            let agent_context = BasicAgentContext::with_shared_messages(
                messages,
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
            let interaction_handle: Arc<dyn InteractionHandle> = Arc::new(
                SharedInteractionHandle::new(resolved.bindings.interaction_handle.clone()),
            );
            let operation_backend: Arc<dyn OperationBackend> =
                Arc::new(PermissionAwareOperationBackend::new(
                    operation_backend,
                    Arc::clone(&interaction_handle),
                    operation_backend_exec_isolation(resolved.operation_backend.as_ref()),
                ));
            let inner = BasicRuntimeView::new(
                ToolStateStoreBuilderImpl::new()
                    .with_config(tool_state_store_config_for_entry_kind(
                        resolved.entry_kind.as_ref(),
                    ))
                    .build()?,
                Box::new(SharedToolEventSink::new(
                    resolved.descriptor.agent_id.clone(),
                    resolved.bindings.tool_event_sink.clone(),
                )),
                trace_recorder,
                Box::new(agent_context),
                Box::new(ArcInteractionHandle::new(interaction_handle)),
                hookers,
                Some(operation_backend.clone()),
            );
            let runtime_view: Arc<dyn RuntimeView> = Arc::new(SkillAwareRuntimeView {
                inner,
                skill_registry: skill_registry.clone(),
                channel_file_sender: resolved.bindings.channel_file_sender.clone(),
            });

            Some(runtime_view)
        };

        let mut builder = Runtime::builder()
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
            tool_manifest,
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

struct SharedToolEventSink {
    agent_id: AgentId,
    inner: Option<Arc<dyn ToolEventSink>>,
}

impl SharedToolEventSink {
    fn new(agent_id: AgentId, inner: Option<Arc<dyn ToolEventSink>>) -> Self {
        Self { agent_id, inner }
    }
}

impl ToolEventSink for SharedToolEventSink {
    fn emit(&self, event: ToolLifecycleEvent) {
        if let Some(inner) = &self.inner {
            inner.emit(event.scoped(self.agent_id.clone()));
            return;
        }
        NoopToolEventSink::new().emit(event);
    }
}

fn operation_backend_exec_isolation(
    config: Option<&crate::backend::GatewayBackendConfig>,
) -> Option<&'static str> {
    let config = config?;
    if config.kind != "local" {
        return None;
    }
    match config
        .options
        .get("isolation")
        .and_then(|value| value.get("kind"))
        .and_then(|value| value.as_str())
    {
        Some("macos_seatbelt") => Some("macos_seatbelt"),
        Some("linux_bubblewrap") => Some("linux_bubblewrap"),
        Some("linux_dynsandbox") => Some("linux_dynsandbox"),
        _ => None,
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

    fn has_builtin_timeout(&self) -> bool {
        self.inner
            .as_ref()
            .is_some_and(|inner| inner.has_builtin_timeout())
    }

    async fn abort_pending(&self, request: &InteractionRequest) {
        if let Some(inner) = &self.inner {
            inner.abort_pending(request).await;
        }
    }
}

struct ArcInteractionHandle {
    inner: Arc<dyn InteractionHandle>,
}

impl ArcInteractionHandle {
    fn new(inner: Arc<dyn InteractionHandle>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl InteractionHandle for ArcInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        self.inner.ask(request).await
    }

    fn has_builtin_timeout(&self) -> bool {
        self.inner.has_builtin_timeout()
    }

    async fn abort_pending(&self, request: &InteractionRequest) {
        self.inner.abort_pending(request).await;
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
    fn operation_backend(&self) -> Option<Arc<dyn OperationBackend>> {
        self.inner.operation_backend()
    }
}
