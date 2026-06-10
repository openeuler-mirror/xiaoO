use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionControlPlane,
    SessionLifecycleStatus, SessionOpenRequest, SessionRecord, SessionRuntimeBuildInput,
    SessionRuntimeResolveError, SessionRuntimeResolver, SessionService, SessionServiceError,
    SessionStore, SessionStoreError,
};
use agent_contracts::{ChannelFileSender, HookerRegistry, InteractionHandle, LoopEventSink};
use agent_types::hook::{HookInvokeInput, HookInvokeMetadata, HookPointId};
use agent_types::session::{SessionClosedHookInput, SessionCreatedHookInput};
use async_trait::async_trait;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use subagent::{
    JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControl, SubagentControlError,
};
use tokio::sync::Mutex;
use xiaoo_core::NoopRuntimeView;

use super::session_backend::{lease_session_backend, sync_session_backend_instance};
use super::session_supervisor::SessionSupervisor;
use crate::gateway::backend::ExternalBackendManager;

pub struct CoreBackedSessionService {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    supervisors: Mutex<HashMap<String, Arc<SessionSupervisor>>>,
    hooker_registry: Arc<dyn HookerRegistry>,
    backend_manager: Arc<ExternalBackendManager>,
}

impl CoreBackedSessionService {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_registry: Arc<dyn HookerRegistry>,
        backend_manager: Arc<ExternalBackendManager>,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            supervisors: Mutex::new(HashMap::new()),
            hooker_registry,
            backend_manager,
        }
    }

    async fn fire_session_hooks(&self, input: HookInvokeInput, hook_point: HookPointId) {
        let hookers: Vec<_> = self
            .hooker_registry
            .list_for_hook_point(&hook_point)
            .into_iter()
            .filter(|h| self.hooker_registry.is_enabled(h.id()))
            .map(|h| h.id().clone())
            .collect();

        let noop_runtime = NoopRuntimeView::new();
        for hooker_id in hookers {
            if let Some(hooker) = self.hooker_registry.get(&hooker_id) {
                if let Err(err) = hooker.invoke(input.clone(), &noop_runtime).await {
                    tracing::warn!(
                        hooker_id = %hooker_id,
                        hook_point = %hook_point.0,
                        error = %err,
                        "session hook invocation failed"
                    );
                }
            }
        }
    }

    async fn get_or_create_supervisor(&self, session: SessionRecord) -> Arc<SessionSupervisor> {
        let mut supervisors = self.supervisors.lock().await;
        if let Some(existing) = supervisors.get(&session.session_id) {
            return existing.clone();
        }

        let supervisor = Arc::new(SessionSupervisor::new(
            self.session_store.clone(),
            self.runtime_resolver.clone(),
            Arc::clone(&self.backend_manager),
            session.clone(),
        ));
        supervisors.insert(session.session_id.clone(), supervisor.clone());
        supervisor
    }

    async fn supervisor_for_session(&self, session_id: &str) -> Option<Arc<SessionSupervisor>> {
        if let Some(existing) = self.supervisors.lock().await.get(session_id).cloned() {
            return Some(existing);
        }

        let session = self.session_store.load(session_id).await?;
        Some(self.get_or_create_supervisor(session).await)
    }

    fn build_session_for_turn(
        request: &AppTurnRequest,
        resolved: &ResolvedSessionRuntime,
    ) -> SessionRecord {
        let now_ms = current_time_ms();
        SessionRecord {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            entry: request.entry.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: resolved.descriptor.agent_id.clone(),
                model: resolved.descriptor.model.clone(),
                system_prompt: resolved.descriptor.system_prompt.clone(),
                feature_flags: resolved.descriptor.feature_flags.clone(),
                token_budget: resolved.descriptor.token_budget.clone(),
                workspace_root: resolved.descriptor.workspace_root.clone(),
                max_turns: resolved.descriptor.max_turns,
                tool_manifest: None,
                subagent_roles: resolved.descriptor.subagent_roles.clone(),
            },
            backend_instance: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    fn build_session_for_open(
        request: &SessionOpenRequest,
        resolved: &ResolvedSessionRuntime,
    ) -> SessionRecord {
        let now_ms = current_time_ms();
        SessionRecord {
            session_id: request.session_id.clone(),
            conversation_id: request.conversation_id.clone(),
            sender_id: request.sender_id.clone(),
            entry: request.entry.clone(),
            channel: request.channel.clone(),
            channel_instance_id: request.channel_instance_id.clone(),
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: resolved.descriptor.agent_id.clone(),
                model: resolved.descriptor.model.clone(),
                system_prompt: resolved.descriptor.system_prompt.clone(),
                feature_flags: resolved.descriptor.feature_flags.clone(),
                token_budget: resolved.descriptor.token_budget.clone(),
                workspace_root: resolved.descriptor.workspace_root.clone(),
                max_turns: resolved.descriptor.max_turns,
                tool_manifest: None,
                subagent_roles: resolved.descriptor.subagent_roles.clone(),
            },
            backend_instance: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }

    async fn run_turn_inner(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let existing = self.session_store.load(&request.session_id).await;
        let is_new_session = existing.is_none();
        let runtime_input = SessionRuntimeBuildInput::from_turn_request(&request);
        let resolved = self
            .runtime_resolver
            .resolve(&runtime_input, existing.as_ref())
            .await?;

        let mut seed_session =
            existing.unwrap_or_else(|| Self::build_session_for_turn(&request, &resolved));
        let backend_lease =
            lease_session_backend(self.backend_manager.as_ref(), &seed_session, &resolved).await?;
        if sync_session_backend_instance(&mut seed_session, &backend_lease) {
            seed_session.updated_at_ms = current_time_ms();
        }

        if is_new_session {
            let hook_point = HookPointId(format!(
                "{}.Session.lifecycle.created",
                resolved.descriptor.agent_id.0
            ));
            self.fire_session_hooks(
                HookInvokeInput::SessionCreated {
                    input: SessionCreatedHookInput {
                        session_id: seed_session.session_id.clone(),
                        sender_id: seed_session.sender_id.clone(),
                    },
                    metadata: HookInvokeMetadata::default(),
                },
                hook_point,
            )
            .await;
        }

        let supervisor = self.get_or_create_supervisor(seed_session).await;
        supervisor.prepare_root_turn(&request, &resolved).await;
        supervisor
            .run_root_turn(
                request,
                resolved,
                event_sink,
                interaction_handle,
                channel_file_sender,
            )
            .await
    }
}

#[async_trait]
impl SessionService for CoreBackedSessionService {
    async fn run_turn(
        &self,
        request: AppTurnRequest,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, None, None, None).await
    }

    async fn run_turn_with_events(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, event_sink, None, None).await
    }

    async fn run_turn_with_interaction(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, event_sink, interaction_handle, channel_file_sender)
            .await
    }
}

#[async_trait]
impl SessionControlPlane for CoreBackedSessionService {
    async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionRecord, SessionServiceError> {
        if let Some(supervisor) = self.supervisor_for_session(&request.session_id).await {
            return Ok(supervisor.snapshot().await);
        }

        let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
        let resolved = self.runtime_resolver.resolve(&runtime_input, None).await?;
        let mut session = Self::build_session_for_open(&request, &resolved);
        let backend_lease =
            lease_session_backend(self.backend_manager.as_ref(), &session, &resolved).await?;
        if sync_session_backend_instance(&mut session, &backend_lease) {
            session.updated_at_ms = current_time_ms();
        }
        self.session_store.save(session.clone()).await;

        let hook_point = HookPointId(format!(
            "{}.Session.lifecycle.created",
            resolved.descriptor.agent_id.0
        ));
        self.fire_session_hooks(
            HookInvokeInput::SessionCreated {
                input: SessionCreatedHookInput {
                    session_id: session.session_id.clone(),
                    sender_id: session.sender_id.clone(),
                },
                metadata: HookInvokeMetadata::default(),
            },
            hook_point,
        )
        .await;

        Ok(self
            .get_or_create_supervisor(session)
            .await
            .snapshot()
            .await)
    }

    async fn resume_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        match self.supervisor_for_session(session_id).await {
            Some(supervisor) => Ok(Some(supervisor.snapshot().await)),
            None => Ok(None),
        }
    }

    async fn force_close_session(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        let closed = if let Some(supervisor) = self.supervisor_for_session(session_id).await {
            supervisor.force_close().await
        } else {
            let Some(mut existing) = self.session_store.load(session_id).await else {
                return Err(SessionServiceError::SessionNotFound {
                    session_id: session_id.to_string(),
                });
            };
            existing.status = SessionLifecycleStatus::Closed;
            existing.updated_at_ms = current_time_ms();
            self.session_store.save(existing.clone()).await;
            existing
        };
        if let Err(error) = self.backend_manager.release_session(session_id).await {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to release session backends"
            );
        }

        let hook_point = HookPointId(format!(
            "{}.Session.lifecycle.closed",
            closed.runtime.agent_id.0
        ));
        self.fire_session_hooks(
            HookInvokeInput::SessionClosed {
                input: SessionClosedHookInput {
                    session_id: closed.session_id.clone(),
                    sender_id: closed.sender_id.clone(),
                },
                metadata: HookInvokeMetadata::default(),
            },
            hook_point,
        )
        .await;

        Ok(closed)
    }
}

#[async_trait]
impl SubagentControl for CoreBackedSessionService {
    async fn spawn(
        &self,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError> {
        let Some(supervisor) = self.supervisor_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        supervisor.spawn_subagent(request).await
    }

    async fn join(
        &self,
        request: JoinSubagentRequest,
    ) -> Result<JoinSubagentResult, SubagentControlError> {
        let Some(supervisor) = self.supervisor_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        supervisor.join_subagent(request).await
    }
}

impl From<SessionRuntimeResolveError> for SessionServiceError {
    fn from(value: SessionRuntimeResolveError) -> Self {
        Self::RuntimeResolve {
            message: value.to_string(),
        }
    }
}

impl From<SessionStoreError> for SessionServiceError {
    fn from(value: SessionStoreError) -> Self {
        Self::SessionStore {
            message: value.to_string(),
        }
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        backend::GatewayBackendConfig, AppBootstrap, GatewayEntryContext, InMemorySessionStore,
        SessionRuntimeBindings, SessionRuntimeDescriptor,
    };
    use agent_contracts::backend::BackendLifecycleState;
    use agent_contracts::{LlmProvider, ProviderCapabilities};
    use agent_types::common::ids::AgentId;
    use agent_types::context::{FeatureFlags, TokenBudgetConfig};
    use agent_types::hook::HookerRegistryConfig;
    use agent_types::{LlmError, LlmRequest, LlmResponse, StreamChunk};
    use llm_client::LlmProviderWrapper;
    use serde_json::{json, Value};
    use tempfile::TempDir;

    struct StubLlmProvider {
        capabilities: ProviderCapabilities,
    }

    #[async_trait]
    impl LlmProvider for StubLlmProvider {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            Err(LlmError::RequestFailed {
                message: "stub provider is not expected to complete in session tests".to_string(),
            })
        }

        async fn complete_stream(
            &self,
            _request: &LlmRequest,
            _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            Err(LlmError::RequestFailed {
                message: "stub provider is not expected to stream in session tests".to_string(),
            })
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }

    struct StubRuntimeResolver {
        workspace_root: std::path::PathBuf,
        backend_options: Value,
        llm_provider: Arc<LlmProviderWrapper>,
    }

    #[async_trait]
    impl SessionRuntimeResolver for StubRuntimeResolver {
        async fn resolve(
            &self,
            request: &SessionRuntimeBuildInput,
            _existing: Option<&SessionRecord>,
        ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
            Ok(ResolvedSessionRuntime {
                descriptor: SessionRuntimeDescriptor {
                    agent_id: AgentId("test-agent".to_string()),
                    model: "stub-model".to_string(),
                    system_prompt: "test system".to_string(),
                    feature_flags: FeatureFlags::default(),
                    token_budget: TokenBudgetConfig {
                        total_budget: 4096,
                        reserved_for_output: 1024,
                        reserved_for_system: 256,
                        hard_limit_ratio: 0.9,
                    },
                    workspace_root: self.workspace_root.clone(),
                    max_turns: None,
                    subagent_roles: BTreeMap::new(),
                },
                entry_kind: request.entry.kind.clone(),
                llm_provider: Arc::clone(&self.llm_provider),
                tool_registry: None,
                skill_registry: None,
                bindings: SessionRuntimeBindings::default(),
                compression_pipeline: None,
                trace: Value::Null,
                hooker: Default::default(),
                operation_backend: Some(GatewayBackendConfig::new(
                    "local",
                    self.backend_options.clone(),
                )),
            })
        }
    }

    fn stub_llm_provider() -> Arc<LlmProviderWrapper> {
        Arc::new(LlmProviderWrapper::new(
            Arc::new(StubLlmProvider {
                capabilities: ProviderCapabilities {
                    supports_streaming: false,
                    supports_tool_calls: false,
                    supports_json_mode: false,
                    max_context_window: 4096,
                    model_name: "stub-model".to_string(),
                },
            }),
            None,
            None,
        ))
    }

    #[tokio::test]
    async fn open_session_persists_active_backend_instance() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(ExternalBackendManager::new()),
        )
        .expect("dependencies");

        let record = dependencies
            .session_control_plane
            .open_session(SessionOpenRequest {
                session_id: "s1".to_string(),
                conversation_id: "c1".to_string(),
                sender_id: "u1".to_string(),
                entry: GatewayEntryContext::tui(None),
                channel: None,
                channel_instance_id: None,
            })
            .await
            .expect("open session");
        let instance = record.backend_instance.expect("backend instance");
        assert_eq!(instance.state, BackendLifecycleState::Active);
        assert_eq!(instance.session_id, "s1");

        let saved = store.load("s1").await.expect("saved session");
        let saved_instance = saved.backend_instance.expect("saved backend instance");
        assert_eq!(saved_instance.state, BackendLifecycleState::Active);
        assert_eq!(saved_instance.backend_id, instance.backend_id);
    }
}
