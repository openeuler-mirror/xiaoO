use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionControlPlane, SessionInput,
    SessionLifecycleStatus, SessionOpenRequest, SessionRecord, SessionRuntimeBuildInput,
    SessionRuntimeResolveError, SessionRuntimeResolver, SessionService, SessionServiceError,
    SessionStore, SessionStoreError,
};
use crate::{
    RuntimeCheckoutRequest, RuntimeCheckoutResult, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, RuntimeCheckpointSnapshotDeleteRequest,
    RuntimeCheckpointSnapshotDeleteResult, RuntimeRecord,
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
use super::session_handle::SessionHandle;
use super::session_supervisor::SessionSupervisor;
use crate::backend::{
    BackendCheckoutRequest, BackendCheckpointRequest, BackendCheckpointSnapshotDeleteRequest,
    BackendError, BackendManager,
};
use crate::runtime_checkpoint::{InMemoryRuntimeCheckpointStore, RuntimeCheckpoint};

pub struct CoreBackedSessionService {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    sessions_handler: Mutex<HashMap<String, SessionHandle>>,
    hooker_registry: Arc<dyn HookerRegistry>,
    backend_manager: Arc<BackendManager>,
    runtime_checkpoints: InMemoryRuntimeCheckpointStore,
}

struct RuntimeCheckpointInternal {
    result: RuntimeCheckpointResult,
    // session: SessionRecord,
    // backend_checkpoint: Option<BackendCheckpointResult>,
}

struct RuntimeCheckoutInternal {
    result: RuntimeCheckoutResult,
    // session: SessionRecord,
    // backend_checkout: Option<BackendCheckoutResult>,
}

struct RuntimeCheckpointSnapshotDeleteInternal {
    result: RuntimeCheckpointSnapshotDeleteResult,
}

impl CoreBackedSessionService {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_registry: Arc<dyn HookerRegistry>,
        backend_manager: Arc<BackendManager>,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            sessions_handler: Mutex::new(HashMap::new()),
            hooker_registry,
            backend_manager,
            runtime_checkpoints: InMemoryRuntimeCheckpointStore::default(),
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

    async fn get_or_create_session_handle(&self, session: SessionRecord) -> SessionHandle {
        if let Some(existing) = self
            .sessions_handler
            .lock()
            .await
            .get(&session.session_id)
            .cloned()
        {
            return existing.clone();
        }

        let supervisor = Arc::new(SessionSupervisor::new(
            self.session_store.clone(),
            self.runtime_resolver.clone(),
            Arc::clone(&self.backend_manager),
            session.clone(),
        ));
        let handle = SessionHandle::new(session.session_id.clone(), supervisor).await;
        let mut sessions = self.sessions_handler.lock().await;
        if let Some(existing) = sessions.get(&session.session_id) {
            return existing.clone();
        }
        sessions.insert(session.session_id.clone(), handle.clone());
        handle
    }

    async fn handle_for_session(&self, session_id: &str) -> Option<SessionHandle> {
        if let Some(existing) = self.sessions_handler.lock().await.get(session_id).cloned() {
            return Some(existing);
        }

        let session = self.session_store.load(session_id).await?;
        Some(self.get_or_create_session_handle(session).await)
    }

    async fn idle_session_snapshot(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        if let Some(handle) = self.sessions_handler.lock().await.get(session_id).cloned() {
            let status = handle.status();
            if status.phase != super::session_handle::SessionPhase::Idle || status.queue_depth > 0 {
                return Err(SessionServiceError::SessionBusy {
                    session_id: session_id.to_string(),
                    message: "runtime must be idle before checkpoint or checkout".to_string(),
                });
            }
            return handle.snapshot().await;
        }

        let Some(session) = self.session_store.load(session_id).await else {
            return Err(SessionServiceError::SessionNotFound {
                session_id: session_id.to_string(),
            });
        };
        if session.status == SessionLifecycleStatus::Running {
            return Err(SessionServiceError::SessionBusy {
                session_id: session_id.to_string(),
                message: "runtime must be idle before checkpoint or checkout".to_string(),
            });
        }
        Ok(session)
    }

    async fn checkpoint_runtime_internal(
        &self,
        request: RuntimeCheckpointRequest,
    ) -> Result<RuntimeCheckpointInternal, SessionServiceError> {
        let session = self.idle_session_snapshot(&request.runtime_id).await?;
        let backend_checkpoint = if let Some(parent_backend) = session.backend_instance.as_ref() {
            Some(
                self.backend_manager
                    .checkpoint_backend(BackendCheckpointRequest {
                        backend_id: Some(parent_backend.backend_id.0.clone()),
                        session_id: Some(request.runtime_id.clone()),
                        name: request.name.clone(),
                        metadata: request.metadata.clone(),
                    })
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!("failed to checkpoint runtime backend: {error}"),
                    })?,
            )
        } else {
            None
        };

        let parent_checkpoint_id = self
            .runtime_checkpoints
            .latest_for_runtime(&request.runtime_id)
            .await;
        let checkpoint_id = format!("rtcp_{}", uuid::Uuid::new_v4().simple());
        let created_at_ms = current_time_ms();
        let checkpoint = RuntimeCheckpoint {
            checkpoint_id: checkpoint_id.clone(),
            runtime_id: request.runtime_id.clone(),
            parent_checkpoint_id: parent_checkpoint_id.clone(),
            session: session.clone(),
            backend_checkpoint: backend_checkpoint
                .as_ref()
                .map(|result| result.checkpoint.clone()),
            created_at_ms,
            metadata: request.metadata.clone(),
            name: request.name.clone(),
        };
        self.runtime_checkpoints.save(checkpoint).await;

        Ok(RuntimeCheckpointInternal {
            result: RuntimeCheckpointResult {
                checkpoint_id,
                runtime: RuntimeRecord::from_session(&session),
                parent_checkpoint_id,
                created_at_ms,
                metadata: request.metadata,
                name: request.name,
            },
            // session,
            // backend_checkpoint,
        })
    }

    async fn checkout_runtime_internal(
        &self,
        request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutInternal, SessionServiceError> {
        let checkpoint = self
            .runtime_checkpoints
            .load(&request.checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: format!("checkpoint:{}", request.checkpoint_id),
            })?;
        let _ = self.idle_session_snapshot(&checkpoint.runtime_id).await?;

        let child_runtime_id = format!(
            "{}:checkout:{}",
            checkpoint.runtime_id,
            uuid::Uuid::new_v4().simple()
        );
        if self.session_store.load(&child_runtime_id).await.is_some() {
            return Err(SessionServiceError::SessionBusy {
                session_id: child_runtime_id,
                message: "generated runtime already exists".to_string(),
            });
        }

        let backend_checkout =
            if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone() {
                Some(
                    self.backend_manager
                        .checkout_backend(BackendCheckoutRequest {
                            checkpoint: backend_checkpoint,
                            backend_id: None,
                            session_id: Some(child_runtime_id.clone()),
                            timeout: None,
                            metadata: request.metadata.clone(),
                            resource_limits: Default::default(),
                            options: None,
                        })
                        .await
                        .map_err(|error| SessionServiceError::RuntimeBuild {
                            message: format!("failed to checkout runtime backend: {error}"),
                        })?,
                )
            } else {
                None
            };
        let backend_lease = if backend_checkout.is_some() {
            Some(
                self.backend_manager
                    .lease_bound_session(&child_runtime_id)
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!("failed to lease checked out backend: {error}"),
                    })?,
            )
        } else {
            None
        };

        let now_ms = current_time_ms();
        let mut child = checkpoint.session.clone();
        child.session_id = child_runtime_id.clone();
        if let Some(conversation_id) = request.conversation_id {
            child.conversation_id = conversation_id;
        }
        if let Some(sender_id) = request.sender_id {
            child.sender_id = sender_id;
        }
        child.status = SessionLifecycleStatus::Idle;
        child.backend_instance = backend_lease.map(|lease| lease.instance());
        child.last_error = None;
        child.created_at_ms = now_ms;
        child.updated_at_ms = now_ms;

        self.session_store.save(child.clone()).await;
        self.runtime_checkpoints
            .register_runtime_head(child.session_id.clone(), checkpoint.checkpoint_id.clone())
            .await;

        let hook_point = HookPointId(format!(
            "{}.Session.lifecycle.created",
            child.runtime.agent_id.0
        ));
        self.fire_session_hooks(
            HookInvokeInput::SessionCreated {
                input: SessionCreatedHookInput {
                    session_id: child.session_id.clone(),
                    sender_id: child.sender_id.clone(),
                },
                metadata: HookInvokeMetadata::default(),
            },
            hook_point,
        )
        .await;
        self.get_or_create_session_handle(child.clone()).await;

        Ok(RuntimeCheckoutInternal {
            result: RuntimeCheckoutResult {
                checkpoint_id: checkpoint.checkpoint_id,
                source_runtime_id: checkpoint.runtime_id,
                runtime: RuntimeRecord::from_session(&child),
            },
            // session: child,
            // backend_checkout,
        })
    }

    async fn delete_checkpoint_snapshot_internal(
        &self,
        request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteInternal, SessionServiceError> {
        let checkpoint = self
            .runtime_checkpoints
            .load(&request.checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: format!("checkpoint:{}", request.checkpoint_id),
            })?;

        let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone() else {
            return Ok(RuntimeCheckpointSnapshotDeleteInternal {
                result: RuntimeCheckpointSnapshotDeleteResult {
                    checkpoint_id: checkpoint.checkpoint_id,
                    runtime_id: checkpoint.runtime_id,
                    provider: None,
                    provider_snapshot_id: None,
                    provider_snapshot_names: Vec::new(),
                    deleted_provider_snapshot: false,
                    deleted_at_ms: current_time_ms(),
                },
            });
        };

        let provider = backend_checkpoint.provider.clone();
        let provider_snapshot_id = backend_checkpoint.provider_snapshot_id.clone();
        let provider_snapshot_names = backend_checkpoint.provider_snapshot_names.clone();
        let delete = self
            .backend_manager
            .delete_checkpoint_snapshot(BackendCheckpointSnapshotDeleteRequest {
                checkpoint: backend_checkpoint,
            })
            .await
            .map_err(|error| match error {
                BackendError::UnsupportedBackend { kind } => {
                    SessionServiceError::UnsupportedCapability {
                        capability: format!("delete_checkpoint_snapshot:{kind}"),
                    }
                }
                error => SessionServiceError::RuntimeBuild {
                    message: format!("failed to delete checkpoint backend snapshot: {error}"),
                },
            })?;

        self.runtime_checkpoints
            .clear_backend_snapshot(&request.checkpoint_id)
            .await;

        Ok(RuntimeCheckpointSnapshotDeleteInternal {
            result: RuntimeCheckpointSnapshotDeleteResult {
                checkpoint_id: request.checkpoint_id,
                runtime_id: checkpoint.runtime_id,
                provider: Some(provider),
                provider_snapshot_id,
                provider_snapshot_names,
                deleted_provider_snapshot: delete.deleted,
                deleted_at_ms: current_time_ms(),
            },
        })
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
                llm: resolved.descriptor.llm.clone(),
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
                llm: resolved.descriptor.llm.clone(),
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
        let backend_updated = sync_session_backend_instance(&mut seed_session, &backend_lease);
        if backend_updated {
            seed_session.updated_at_ms = current_time_ms();
        }
        if is_new_session || backend_updated {
            self.session_store.save(seed_session.clone()).await;
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

        let handle = self.get_or_create_session_handle(seed_session).await;
        handle
            .run_turn(
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
        if let Some(handle) = self.handle_for_session(&request.session_id).await {
            return handle.snapshot().await;
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

        self.get_or_create_session_handle(session)
            .await
            .snapshot()
            .await
    }

    async fn resume_session(
        &self,
        session_id: &str,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        match self.handle_for_session(session_id).await {
            Some(handle) => Ok(Some(handle.snapshot().await?)),
            None => Ok(None),
        }
    }

    async fn force_close_session(
        &self,
        session_id: &str,
    ) -> Result<SessionRecord, SessionServiceError> {
        let (closed, was_already_closed) =
            if let Some(handle) = self.handle_for_session(session_id).await {
                let before = handle.snapshot().await?;
                let was_already_closed = before.status == SessionLifecycleStatus::Closed;
                (handle.force_close().await?, was_already_closed)
            } else {
                let Some(mut existing) = self.session_store.load(session_id).await else {
                    return Err(SessionServiceError::SessionNotFound {
                        session_id: session_id.to_string(),
                    });
                };
                let was_already_closed = existing.status == SessionLifecycleStatus::Closed;
                existing.status = SessionLifecycleStatus::Closed;
                existing.updated_at_ms = current_time_ms();
                self.session_store.save(existing.clone()).await;
                (existing, was_already_closed)
            };
        if let Err(error) = self.backend_manager.release_session(session_id).await {
            tracing::warn!(
                session_id = %session_id,
                error = %error,
                "failed to release session backends"
            );
        }

        if !was_already_closed {
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
        }

        self.session_store.delete(session_id).await;
        self.sessions_handler.lock().await.remove(session_id);

        Ok(closed)
    }

    async fn checkpoint_runtime(
        &self,
        request: RuntimeCheckpointRequest,
    ) -> Result<RuntimeCheckpointResult, SessionServiceError> {
        self.checkpoint_runtime_internal(request)
            .await
            .map(|internal| internal.result)
    }

    async fn checkout_runtime(
        &self,
        request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutResult, SessionServiceError> {
        self.checkout_runtime_internal(request)
            .await
            .map(|internal| internal.result)
    }

    async fn delete_checkpoint_snapshot(
        &self,
        request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteResult, SessionServiceError> {
        self.delete_checkpoint_snapshot_internal(request)
            .await
            .map(|internal| internal.result)
    }

    async fn submit_input(
        &self,
        session_id: &str,
        input: SessionInput,
    ) -> Result<crate::gateway::SessionSubmitReceipt, SessionServiceError> {
        match input {
            SessionInput::CancelActiveTurn => {
                let Some(handle) = self.handle_for_session(session_id).await else {
                    return Err(SessionServiceError::SessionNotFound {
                        session_id: session_id.to_string(),
                    });
                };
                handle.cancel_active_turn().await
            }
            SessionInput::Turn { .. } => Err(SessionServiceError::UnsupportedCapability {
                capability: "submit_input.turn".to_string(),
            }),
            SessionInput::Interaction { .. } => Err(SessionServiceError::UnsupportedCapability {
                capability: "submit_input.interaction".to_string(),
            }),
            SessionInput::InputChunk { .. } => Err(SessionServiceError::UnsupportedCapability {
                capability: "submit_input.input_chunk".to_string(),
            }),
        }
    }
}

#[async_trait]
impl SubagentControl for CoreBackedSessionService {
    async fn spawn(
        &self,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError> {
        let Some(handle) = self.handle_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        let supervisor = handle.supervisor();
        supervisor.spawn_subagent(request).await
    }

    async fn join(
        &self,
        request: JoinSubagentRequest,
    ) -> Result<JoinSubagentResult, SubagentControlError> {
        let Some(handle) = self.handle_for_session(&request.session_id).await else {
            return Err(SubagentControlError::Unavailable {
                message: format!("session '{}' is not available", request.session_id),
            });
        };
        let supervisor = handle.supervisor();
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
    use crate::backend::GatewayBackendConfig;
    use crate::gateway::{
        AppBootstrap, GatewayEntryContext, InMemorySessionStore, SessionInput, SessionInputKind,
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
                    llm: None,
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

    fn test_open_request(session_id: &str) -> SessionOpenRequest {
        SessionOpenRequest {
            session_id: session_id.to_string(),
            conversation_id: format!("{session_id}-conversation"),
            sender_id: "user-1".to_string(),
            entry: GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
            llm: None,
        }
    }

    async fn save_session_without_backend(
        store: &Arc<InMemorySessionStore>,
        resolver: &Arc<StubRuntimeResolver>,
        session_id: &str,
        status: SessionLifecycleStatus,
    ) -> SessionRecord {
        let request = test_open_request(session_id);
        let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
        let resolved = resolver
            .resolve(&runtime_input, None)
            .await
            .expect("resolve runtime");
        let mut session = CoreBackedSessionService::build_session_for_open(&request, &resolved);
        session.status = status;
        session.backend_instance = None;
        store.save(session.clone()).await;
        session
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
            Arc::new(BackendManager::new()),
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
                llm: None,
            })
            .await
            .expect("open session");
        let instance = record.backend_instance.expect("backend instance");
        assert_eq!(instance.state, BackendLifecycleState::Active);
        assert_eq!(instance.session_id, "s1");
        assert!(instance.backend_id.0.starts_with("bkd_"));
        assert_ne!(instance.backend_id.0, "s1");

        let saved = store.load("s1").await.expect("saved session");
        let saved_instance = saved.backend_instance.expect("saved backend instance");
        assert_eq!(saved_instance.state, BackendLifecycleState::Active);
        assert_eq!(saved_instance.backend_id, instance.backend_id);
    }

    #[tokio::test]
    async fn submit_cancel_active_turn_routes_through_session_handle() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store,
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        dependencies
            .session_control_plane
            .open_session(SessionOpenRequest {
                session_id: "s-cancel".to_string(),
                conversation_id: "c1".to_string(),
                sender_id: "u1".to_string(),
                entry: GatewayEntryContext::tui(None),
                channel: None,
                channel_instance_id: None,
                llm: None,
            })
            .await
            .expect("open session");

        let receipt = dependencies
            .session_control_plane
            .submit_input("s-cancel", SessionInput::CancelActiveTurn)
            .await
            .expect("cancel should be accepted");

        assert_eq!(receipt.session_id, "s-cancel");
        assert_eq!(receipt.accepted_kind, SessionInputKind::CancelActiveTurn);
    }

    #[tokio::test]
    async fn force_close_session_removes_session_record() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store,
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        dependencies
            .session_control_plane
            .open_session(SessionOpenRequest {
                session_id: "s-close".to_string(),
                conversation_id: "c1".to_string(),
                sender_id: "u1".to_string(),
                entry: GatewayEntryContext::tui(None),
                channel: None,
                channel_instance_id: None,
                llm: None,
            })
            .await
            .expect("open session");

        let closed = dependencies
            .session_control_plane
            .force_close_session("s-close")
            .await
            .expect("close session");
        assert_eq!(closed.status, SessionLifecycleStatus::Closed);

        let resumed = dependencies
            .session_control_plane
            .resume_session("s-close")
            .await
            .expect("resume closed session");
        assert!(resumed.is_none());
    }

    #[tokio::test]
    async fn checkpoint_runtime_idle_session_without_backend_succeeds() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        save_session_without_backend(&store, &resolver, "runtime-1", SessionLifecycleStatus::Idle)
            .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store,
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        let result = dependencies
            .session_control_plane
            .checkpoint_runtime(RuntimeCheckpointRequest {
                runtime_id: "runtime-1".to_string(),
                metadata: json!({"kind": "test"}),
                name: Some("checkpoint-a".to_string()),
            })
            .await
            .expect("checkpoint runtime");

        assert!(result.checkpoint_id.starts_with("rtcp_"));
        assert_eq!(result.runtime.runtime_id, "runtime-1");
        assert_eq!(result.parent_checkpoint_id, None);
        assert_eq!(result.metadata, json!({"kind": "test"}));
        assert_eq!(result.name.as_deref(), Some("checkpoint-a"));
    }

    #[tokio::test]
    async fn delete_checkpoint_snapshot_without_backend_snapshot_is_noop() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        save_session_without_backend(&store, &resolver, "runtime-1", SessionLifecycleStatus::Idle)
            .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store,
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");
        let checkpoint = dependencies
            .session_control_plane
            .checkpoint_runtime(RuntimeCheckpointRequest {
                runtime_id: "runtime-1".to_string(),
                metadata: Value::Null,
                name: None,
            })
            .await
            .expect("checkpoint runtime");

        let result = dependencies
            .session_control_plane
            .delete_checkpoint_snapshot(RuntimeCheckpointSnapshotDeleteRequest {
                checkpoint_id: checkpoint.checkpoint_id.clone(),
            })
            .await
            .expect("delete checkpoint snapshot");

        assert_eq!(result.checkpoint_id, checkpoint.checkpoint_id);
        assert_eq!(result.runtime_id, "runtime-1");
        assert_eq!(result.provider, None);
        assert_eq!(result.provider_snapshot_id, None);
        assert!(!result.deleted_provider_snapshot);
    }

    #[tokio::test]
    async fn checkout_runtime_creates_new_runtime_from_checkpoint() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let parent = save_session_without_backend(
            &store,
            &resolver,
            "runtime-parent",
            SessionLifecycleStatus::Idle,
        )
        .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");
        let checkpoint = dependencies
            .session_control_plane
            .checkpoint_runtime(RuntimeCheckpointRequest {
                runtime_id: parent.session_id.clone(),
                metadata: Value::Null,
                name: None,
            })
            .await
            .expect("checkpoint runtime");

        let checkout = dependencies
            .session_control_plane
            .checkout_runtime(RuntimeCheckoutRequest {
                checkpoint_id: checkpoint.checkpoint_id.clone(),
                conversation_id: Some("child-conversation".to_string()),
                sender_id: Some("child-user".to_string()),
                metadata: json!({"branch": "a"}),
            })
            .await
            .expect("checkout runtime");

        assert_eq!(checkout.checkpoint_id, checkpoint.checkpoint_id);
        assert_eq!(checkout.source_runtime_id, parent.session_id);
        assert_ne!(checkout.runtime.runtime_id, parent.session_id);
        assert!(checkout
            .runtime
            .runtime_id
            .starts_with("runtime-parent:checkout:"));
        assert_eq!(checkout.runtime.conversation_id, "child-conversation");
        assert_eq!(checkout.runtime.sender_id, "child-user");

        let saved = store
            .load(&checkout.runtime.runtime_id)
            .await
            .expect("checked out runtime saved");
        assert_eq!(saved.conversation_id, "child-conversation");
        assert_eq!(saved.sender_id, "child-user");
        assert_eq!(saved.status, SessionLifecycleStatus::Idle);
        assert!(saved.backend_instance.is_none());
        assert_eq!(saved.runtime.agent_id, parent.runtime.agent_id);
    }

    #[tokio::test]
    async fn checkpoint_runtime_rejects_running_session() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        save_session_without_backend(
            &store,
            &resolver,
            "runtime-running",
            SessionLifecycleStatus::Running,
        )
        .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store,
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        let result = dependencies
            .session_control_plane
            .checkpoint_runtime(RuntimeCheckpointRequest {
                runtime_id: "runtime-running".to_string(),
                metadata: Value::Null,
                name: None,
            })
            .await;

        assert!(matches!(
            result,
            Err(SessionServiceError::SessionBusy { session_id, .. })
                if session_id == "runtime-running"
        ));
    }

    #[tokio::test]
    async fn checkout_runtime_rejects_running_source_runtime() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let mut parent = save_session_without_backend(
            &store,
            &resolver,
            "runtime-source",
            SessionLifecycleStatus::Idle,
        )
        .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");
        let checkpoint = dependencies
            .session_control_plane
            .checkpoint_runtime(RuntimeCheckpointRequest {
                runtime_id: "runtime-source".to_string(),
                metadata: Value::Null,
                name: None,
            })
            .await
            .expect("checkpoint runtime");
        parent.status = SessionLifecycleStatus::Running;
        store.save(parent).await;

        let result = dependencies
            .session_control_plane
            .checkout_runtime(RuntimeCheckoutRequest {
                checkpoint_id: checkpoint.checkpoint_id,
                conversation_id: None,
                sender_id: None,
                metadata: Value::Null,
            })
            .await;

        assert!(matches!(
            result,
            Err(SessionServiceError::SessionBusy { session_id, .. })
                if session_id == "runtime-source"
        ));
    }
}
