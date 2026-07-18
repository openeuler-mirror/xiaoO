use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionControlPlane, SessionInput,
    SessionLifecycleStatus, SessionOpenRequest, SessionRecord, SessionRuntimeBuildInput,
    SessionRuntimeResolveError, SessionRuntimeResolver, SessionService, SessionServiceError,
    SessionStore, SessionStoreError,
};
use crate::{
    RuntimeCheckoutRequest, RuntimeCheckoutResult, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, RuntimeCheckpointSnapshotDeleteRequest,
    RuntimeCheckpointSnapshotDeleteResult, RuntimeExecRequest, RuntimeExecResult,
    RuntimePauseRequest, RuntimePauseResult, RuntimeReadFileRequest, RuntimeReadFileResult,
    RuntimeRecord, RuntimeResumeRequest, RuntimeResumeResult, RuntimeWriteFileRequest,
    RuntimeWriteFileResult,
};
use agent_contracts::backend::{
    capability::{
        exec::ExecRequest,
        filesystem::{ReadBytesRequest, WriteBytesRequest, WriteMode},
    },
    BackendPath,
};
use agent_contracts::{ChannelFileSender, HookerRegistry, InteractionHandle, LoopEventSink};
use agent_types::common::HookerId;
use agent_types::hook::{HookAction, HookInvokeInput, HookInvokeMetadata, HookPointId};
use agent_types::session::{
    SessionClosedHookInput, SessionCreatedHookInput, SessionStateHookInput,
};
use async_trait::async_trait;
use base64::Engine as _;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use subagent::{
    JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControl, SubagentControlError,
};
use tokio::sync::Mutex;
use xiaoo_core::NoopRuntimeView;

use super::session_backend::{
    checkout_backend_with_eviction, lease_session_backend, sync_session_backend_instance,
    CheckoutEvictionContext,
};
use super::session_handle::SessionHandle;
use super::session_supervisor::SessionSupervisor;
use crate::backend::{
    BackendCheckoutRequest, BackendCheckpointRequest, BackendCheckpointSnapshotDeleteRequest,
    BackendError, BackendLease, BackendManager,
};
use crate::runtime_checkpoint::{InMemoryRuntimeCheckpointStore, RuntimeCheckpoint};

/// Overall wall-clock budget for collecting `*.Session.lifecycle.state`
/// hook actions before the turn's `Done` event is emitted. Bounded at one
/// hooker's per-subprocess cap (`PLUGIN_HOOK_COMMAND_TIMEOUT_MS = 30s`) so a
/// single legitimately slow hooker is unaffected, while the sum across N
/// hookers is capped at 30s instead of N × 30s. On timeout the spawned task
/// is aborted (its in-flight subprocess is reaped via `kill_on_drop`) and no
/// actions are returned.
const SESSION_STATE_HOOK_OVERALL_DEADLINE: tokio::time::Duration =
    tokio::time::Duration::from_secs(30);
const RUNTIME_EXEC_FALLBACK_SHELL: &str = "/bin/sh";

fn resolve_runtime_exec_shell(requested: Option<String>, backend_default: Option<&str>) -> String {
    requested
        .or_else(|| backend_default.map(str::to_string))
        .unwrap_or_else(|| RUNTIME_EXEC_FALLBACK_SHELL.to_string())
}

pub struct CoreBackedSessionService {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    sessions_handler: Mutex<HashMap<String, SessionHandle>>,
    runtime_initialization_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    hooker_registry: Arc<dyn HookerRegistry>,
    backend_manager: Arc<BackendManager>,
    runtime_checkpoints: InMemoryRuntimeCheckpointStore,
    /// Cross-turn `send_prompt` chain depth cap (exclusive upper bound on
    /// `chain_depth`). Stamped onto each `SendPrompt` action
    /// (`emitting turn depth + 1`) by
    /// [`fire_session_state_hook_and_collect_actions`]; actions whose
    /// stamped `chain_depth` would **reach** this value
    /// (`next_depth >= max_prompt_chain_depth`) are dropped before
    /// forwarding. Semantics: `N` permits **N turns total** in a chain —
    /// the user-initiated turn (depth `0`) plus `N - 1`
    /// `send_prompt`-triggered turns (depths `1..=N-1`); a `send_prompt`
    /// that would start the `N`-th-turn-after-user (depth `N`) is dropped.
    /// Defaults to
    /// [`DEFAULT_MAX_PROMPT_CHAIN_DEPTH`](agent_types::hook::DEFAULT_MAX_PROMPT_CHAIN_DEPTH)
    /// (128); configurable via `[hooker].max_prompt_chain_depth`.
    max_prompt_chain_depth: usize,
}

impl CoreBackedSessionService {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_registry: Arc<dyn HookerRegistry>,
        backend_manager: Arc<BackendManager>,
        max_prompt_chain_depth: usize,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            sessions_handler: Mutex::new(HashMap::new()),
            runtime_initialization_locks: Mutex::new(HashMap::new()),
            hooker_registry,
            backend_manager,
            runtime_checkpoints: InMemoryRuntimeCheckpointStore::default(),
            max_prompt_chain_depth,
        }
    }

    async fn runtime_initialization_lock(&self, runtime_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.runtime_initialization_locks.lock().await;
        // The map itself owns one strong reference. Drop entries with no
        // active/waiting caller so arbitrary runtime IDs cannot grow it forever.
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(
            locks
                .entry(runtime_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(()))),
        )
    }

    async fn fire_session_hooks(&self, input: HookInvokeInput, hook_point: HookPointId) {
        let hookers = self.enabled_hooker_ids_for(&hook_point);

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

    /// Fire the `*.Session.lifecycle.state` event hook, await all hookers,
    /// and collect the side-effect `actions` they request.
    ///
    /// Used by the daemon's `stream_session_input` path (via `run_turn_inner`)
    /// so that plugin-requested actions are bundled into the turn's `Done`
    /// event — the TUI processes them when it receives `Done`. The trade-off
    /// vs a fire-and-forget design is that the user sees the turn result after
    /// the hookers finish (or time out), which is acceptable for session
    /// lifecycle hooks that only fire after turn termination anyway.
    ///
    /// An overall deadline of [`SESSION_STATE_HOOK_OVERALL_DEADLINE`] (30s)
    /// bounds the collection regardless of how many hookers are registered:
    /// a single hooker can still take up to its 30s per-subprocess cap (no
    /// regression), but the sum across N hookers is capped at 30s instead of
    /// growing to N × 30s. On timeout the spawned task is aborted (the
    /// in-flight subprocess is reaped via `kill_on_drop`) and no actions are
    /// returned — best-effort, matching the documented semantics that action
    /// failures never propagate to the hook caller.
    ///
    /// Actions are returned raw: daemon-side execution (e.g. `open_session`
    /// for `CreateSession`/`SwitchSession`) is the caller's responsibility
    /// (the HTTP router does this via its own `DaemonHookActionSink`).
    ///
    /// The hooker invocations run inside a `tokio::spawn`d task and the
    /// `JoinHandle` is awaited here (under the overall deadline). We still
    /// block on the hookers (so the requested actions are collected before
    /// `Done` is emitted) but a panicking or cancelled plugin task surfaces
    /// as a `JoinError` here rather than unwinding into `run_turn_inner` and
    /// tearing down the daemon's SSE connection. A `JoinError` yields an
    /// empty action set so the turn result is still delivered.
    pub async fn fire_session_state_hook_and_collect_actions(
        &self,
        session_id: String,
        sender_id: String,
        agent_id: String,
        state: String,
        outcome: String,
        emitting_turn_chain_depth: usize,
    ) -> Vec<HookAction> {
        let hook_point = session_lifecycle_hook_point(&agent_id, "state");
        let hooker_ids = self.enabled_hooker_ids_for(&hook_point);
        if hooker_ids.is_empty() {
            return Vec::new();
        }

        let max_depth = self.max_prompt_chain_depth;
        let registry = Arc::clone(&self.hooker_registry);
        let mut hook_task = tokio::spawn(async move {
            let noop_runtime = NoopRuntimeView::new();
            let input = HookInvokeInput::SessionState {
                input: SessionStateHookInput {
                    session_id,
                    sender_id,
                    agent_id,
                    state,
                    outcome,
                },
                metadata: HookInvokeMetadata::default(),
            };

            let mut all_actions = Vec::new();
            for hooker_id in hooker_ids {
                let Some(hooker) = registry.get(&hooker_id) else {
                    continue;
                };
                match hooker.invoke(input.clone(), &noop_runtime).await {
                    Ok(invoke_output) => {
                        all_actions.extend(invoke_output.actions);
                    }
                    Err(error) => {
                        tracing::warn!(
                            hooker_id = %hooker_id,
                            hook_point = "session.lifecycle.state",
                            error = %error,
                            "session state hook invocation failed"
                        );
                    }
                }
            }
            all_actions
        });

        let deadline_sleep = tokio::time::sleep(SESSION_STATE_HOOK_OVERALL_DEADLINE);
        tokio::pin!(deadline_sleep);
        let collected: Vec<HookAction> = tokio::select! {
            task_result = &mut hook_task => match task_result {
                Ok(actions) => actions,
                Err(join_error) => {
                    tracing::warn!(
                        hook_point = "session.lifecycle.state",
                        error = %join_error,
                        "session state hook task did not complete \
                         (panic or runtime shutdown); returning no actions"
                    );
                    Vec::new()
                }
            },
            _ = &mut deadline_sleep => {
                tracing::warn!(
                    hook_point = "session.lifecycle.state",
                    deadline_secs = SESSION_STATE_HOOK_OVERALL_DEADLINE.as_secs(),
                    "session state hook collection exceeded overall deadline; \
                     aborting pending hookers and returning no actions"
                );
                hook_task.abort();
                Vec::new()
            }
        };

        // Stamp and cap `SendPrompt` actions before forwarding. The emitting
        // turn's depth is known here (the turn just finished); each surviving
        // `SendPrompt` carries `chain_depth = emitting_turn_depth + 1` so the
        // TUI can relay it back via `RuntimeTurnRequest.chain_depth`, letting
        // the daemon track the resulting turn's depth and re-enforce the cap
        // when that turn ends. Plugin-supplied `chain_depth` values are
        // overwritten unconditionally — plugins cannot forge a low depth to
        // bypass the cap. A normal user-typed turn carries `chain_depth = 0`,
        // which resets the chain.
        stamp_and_cap_send_prompt_actions(collected, emitting_turn_chain_depth, max_depth)
    }

    /// Collect the (id)s of enabled hookers registered for `hook_point`,
    /// sorted by id for a stable execution order. Shared by both
    /// [`fire_session_hooks`] and [`fire_session_state_hook_and_collect_actions`].
    fn enabled_hooker_ids_for(&self, hook_point: &HookPointId) -> Vec<HookerId> {
        let mut ids: Vec<HookerId> = self
            .hooker_registry
            .list_for_hook_point(hook_point)
            .into_iter()
            .filter(|h| self.hooker_registry.is_enabled(h.id()))
            .map(|h| h.id().clone())
            .collect();
        ids.sort_by(|a, b| a.0.cmp(&b.0));
        ids
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
        if session.status == SessionLifecycleStatus::Paused {
            return None;
        }
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

    fn map_backend_error(
        context: &str,
        session_id: &str,
        error: BackendError,
    ) -> SessionServiceError {
        match error {
            BackendError::ResourceLimitExceeded { message } => SessionServiceError::SessionBusy {
                session_id: session_id.to_string(),
                message,
            },
            error => SessionServiceError::RuntimeBuild {
                message: format!("{context}: {error}"),
            },
        }
    }

    async fn lease_bound_backend_for_idle_runtime(
        &self,
        runtime_id: &str,
    ) -> Result<BackendLease, SessionServiceError> {
        let session = self.idle_session_snapshot(runtime_id).await?;
        if session.status == SessionLifecycleStatus::Closed {
            return Err(SessionServiceError::SessionClosed {
                session_id: runtime_id.to_string(),
            });
        }
        self.backend_manager
            .lease_bound_session(runtime_id)
            .await
            .map_err(|error| Self::map_runtime_backend_error(runtime_id, error))
    }

    fn map_runtime_backend_error(runtime_id: &str, error: BackendError) -> SessionServiceError {
        match error {
            BackendError::NotFound { .. } => SessionServiceError::SessionNotFound {
                session_id: runtime_id.to_string(),
            },
            BackendError::UnsupportedBackend { kind } => {
                SessionServiceError::UnsupportedCapability {
                    capability: format!("runtime backend: {kind}"),
                }
            }
            error => SessionServiceError::RuntimeBuild {
                message: format!("runtime backend operation failed: {error}"),
            },
        }
    }

    async fn checkpoint_runtime_internal(
        &self,
        request: RuntimeCheckpointRequest,
    ) -> Result<RuntimeCheckpointResult, SessionServiceError> {
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

        Ok(RuntimeCheckpointResult {
            checkpoint_id,
            runtime: RuntimeRecord::from_session(&session),
            parent_checkpoint_id,
            created_at_ms,
            metadata: request.metadata,
            name: request.name,
        })
    }

    async fn checkout_runtime_internal(
        &self,
        request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutResult, SessionServiceError> {
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

        let backend_lease = if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone()
        {
            Some(
                checkout_backend_with_eviction(
                    self.backend_manager.as_ref(),
                    &child_runtime_id,
                    backend_checkpoint,
                    self.session_store.clone(),
                    request.metadata.clone(),
                    &CheckoutEvictionContext::runtime_checkout(),
                )
                .await?,
            )
        } else {
            None
        };

        let now_ms = current_time_ms();
        let mut child = checkpoint.session.clone();
        child.session_id = child_runtime_id.clone();
        child.parent_runtime_id = Some(checkpoint.runtime_id.clone());
        child.forked_from_checkpoint_id = Some(checkpoint.checkpoint_id.clone());
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

        let hook_point = session_lifecycle_hook_point(&child.runtime.agent_id.0, "created");
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

        Ok(RuntimeCheckoutResult {
            checkpoint_id: checkpoint.checkpoint_id,
            source_runtime_id: checkpoint.runtime_id,
            runtime: RuntimeRecord::from_session(&child),
        })
    }

    async fn pause_runtime_internal(
        &self,
        request: RuntimePauseRequest,
    ) -> Result<RuntimePauseResult, SessionServiceError> {
        let session = self.idle_session_snapshot(&request.runtime_id).await?;
        if session.status == SessionLifecycleStatus::Closed {
            return Err(SessionServiceError::SessionClosed {
                session_id: request.runtime_id,
            });
        }
        if session.status == SessionLifecycleStatus::Paused {
            return Err(SessionServiceError::SessionBusy {
                session_id: request.runtime_id,
                message: "runtime is already paused".to_string(),
            });
        }

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
                        message: format!(
                            "failed to checkpoint runtime backend before pause: {error}"
                        ),
                    })?,
            )
        } else {
            None
        };

        self.backend_manager
            .release_session(&request.runtime_id)
            .await
            .map_err(|error| SessionServiceError::RuntimeShutdown {
                message: format!("failed to release runtime backend during pause: {error}"),
            })?;

        let parent_checkpoint_id = self
            .runtime_checkpoints
            .latest_for_runtime(&request.runtime_id)
            .await;
        let checkpoint_id = format!("rtcp_{}", uuid::Uuid::new_v4().simple());
        let created_at_ms = current_time_ms();
        let mut paused = session.clone();
        paused.status = SessionLifecycleStatus::Paused;
        paused.backend_instance = None;
        paused.last_error = None;
        paused.updated_at_ms = created_at_ms;

        let checkpoint = RuntimeCheckpoint {
            checkpoint_id: checkpoint_id.clone(),
            runtime_id: request.runtime_id.clone(),
            parent_checkpoint_id,
            session: paused.clone(),
            backend_checkpoint: backend_checkpoint
                .as_ref()
                .map(|result| result.checkpoint.clone()),
            created_at_ms,
            metadata: request.metadata.clone(),
            name: request.name.clone(),
        };
        self.runtime_checkpoints.save(checkpoint).await;
        self.runtime_checkpoints
            .register_paused_runtime(request.runtime_id.clone(), checkpoint_id.clone())
            .await;
        self.session_store.save(paused.clone()).await;
        self.sessions_handler
            .lock()
            .await
            .remove(&request.runtime_id);

        Ok(RuntimePauseResult {
            runtime: RuntimeRecord::from_session(&paused),
            checkpoint_id,
            created_at_ms,
            metadata: request.metadata,
            name: request.name,
        })
    }

    async fn resume_runtime_internal(
        &self,
        request: RuntimeResumeRequest,
    ) -> Result<RuntimeResumeResult, SessionServiceError> {
        let mut session = self
            .session_store
            .load(&request.runtime_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: request.runtime_id.clone(),
            })?;
        if session.status == SessionLifecycleStatus::Closed {
            return Err(SessionServiceError::SessionClosed {
                session_id: request.runtime_id,
            });
        }
        if session.status != SessionLifecycleStatus::Paused {
            return Err(SessionServiceError::SessionBusy {
                session_id: request.runtime_id,
                message: "runtime is not paused".to_string(),
            });
        }

        let checkpoint_id = self
            .runtime_checkpoints
            .paused_checkpoint_for_runtime(&request.runtime_id)
            .await
            .ok_or_else(|| SessionServiceError::RuntimeBuild {
                message: format!("paused runtime {} has no checkpoint", request.runtime_id),
            })?;
        let checkpoint = self
            .runtime_checkpoints
            .load(&checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::RuntimeBuild {
                message: format!("paused runtime checkpoint not found: {checkpoint_id}"),
            })?;

        let backend_checkout =
            if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone() {
                Some(
                    self.backend_manager
                        .checkout_backend(BackendCheckoutRequest {
                            checkpoint: backend_checkpoint,
                            backend_id: None,
                            session_id: Some(request.runtime_id.clone()),
                            timeout: None,
                            metadata: request.metadata,
                            resource_limits: Default::default(),
                            options: None,
                        })
                        .await
                        .map_err(|error| {
                            Self::map_backend_error(
                                "failed to resume runtime backend",
                                &request.runtime_id,
                                error,
                            )
                        })?,
                )
            } else {
                None
            };
        let backend_lease = if backend_checkout.is_some() {
            Some(
                self.backend_manager
                    .lease_bound_session(&request.runtime_id)
                    .await
                    .map_err(|error| SessionServiceError::RuntimeBuild {
                        message: format!("failed to lease resumed backend: {error}"),
                    })?,
            )
        } else {
            None
        };

        session.status = SessionLifecycleStatus::Idle;
        session.backend_instance = backend_lease.map(|lease| lease.instance());
        session.last_error = None;
        session.updated_at_ms = current_time_ms();
        self.session_store.save(session.clone()).await;
        self.runtime_checkpoints
            .clear_paused_runtime(&request.runtime_id)
            .await;
        self.runtime_checkpoints
            .register_runtime_head(session.session_id.clone(), checkpoint_id)
            .await;
        self.get_or_create_session_handle(session.clone()).await;

        Ok(RuntimeResumeResult {
            runtime: RuntimeRecord::from_session(&session),
        })
    }

    async fn delete_checkpoint_snapshot_internal(
        &self,
        request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteResult, SessionServiceError> {
        let checkpoint = self
            .runtime_checkpoints
            .load(&request.checkpoint_id)
            .await
            .ok_or_else(|| SessionServiceError::SessionNotFound {
                session_id: format!("checkpoint:{}", request.checkpoint_id),
            })?;

        let Some(backend_checkpoint) = checkpoint.backend_checkpoint.clone() else {
            return Ok(RuntimeCheckpointSnapshotDeleteResult {
                checkpoint_id: checkpoint.checkpoint_id,
                runtime_id: checkpoint.runtime_id,
                provider: None,
                provider_snapshot_id: None,
                provider_snapshot_names: Vec::new(),
                deleted_provider_snapshot: false,
                deleted_at_ms: current_time_ms(),
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

        Ok(RuntimeCheckpointSnapshotDeleteResult {
            checkpoint_id: request.checkpoint_id,
            runtime_id: checkpoint.runtime_id,
            provider: Some(provider),
            provider_snapshot_id,
            provider_snapshot_names,
            deleted_provider_snapshot: delete.deleted,
            deleted_at_ms: current_time_ms(),
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
                bootstrap_binding: resolved.bootstrap_binding.clone(),
            },
            backend_instance: None,
            paused_backend_checkpoint: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            parent_runtime_id: None,
            forked_from_checkpoint_id: None,
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
                bootstrap_binding: resolved.bootstrap_binding.clone(),
            },
            backend_instance: None,
            paused_backend_checkpoint: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            parent_runtime_id: None,
            forked_from_checkpoint_id: None,
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
        cancellation_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let hooks_enabled = !matches!(
            (
                request.entry.kind.as_ref(),
                request.entry.instance_id.as_deref()
            ),
            (Some(crate::gateway::GatewayEntryKind::Mcp), Some("chatbot"))
        );
        let initialization_lock = self.runtime_initialization_lock(&request.session_id).await;
        let initialization_guard = initialization_lock.lock().await;
        let existing = self.session_store.load(&request.session_id).await;
        let is_new_session = existing.is_none();
        let runtime_input = SessionRuntimeBuildInput::from_turn_request(&request);
        let mut resolved = self
            .runtime_resolver
            .resolve(&runtime_input, existing.as_ref())
            .await?;
        resolved.bindings.cancel_token = cancellation_token;

        let mut seed_session =
            existing.unwrap_or_else(|| Self::build_session_for_turn(&request, &resolved));
        let was_paused = seed_session.status == SessionLifecycleStatus::Paused;
        let backend_lease = lease_session_backend(
            self.backend_manager.as_ref(),
            &seed_session,
            &resolved,
            self.session_store.clone(),
        )
        .await?;
        if let Err(error) =
            crate::gateway::finalize_e2b_runtime(&mut resolved, backend_lease.backend()).await
        {
            if is_new_session {
                self.backend_manager
                    .release_session(&request.session_id)
                    .await
                    .ok();
            }
            return Err(error);
        }
        seed_session.runtime.system_prompt = resolved.descriptor.system_prompt.clone();
        seed_session.runtime.workspace_root = resolved.descriptor.workspace_root.clone();
        seed_session.runtime.bootstrap_binding = resolved.bootstrap_binding.clone();
        let backend_updated = sync_session_backend_instance(&mut seed_session, &backend_lease);
        if was_paused {
            seed_session.status = SessionLifecycleStatus::Idle;
            seed_session.paused_backend_checkpoint = None;
            seed_session.last_error = None;
        }
        if backend_updated || was_paused {
            seed_session.updated_at_ms = current_time_ms();
        }
        if is_new_session || backend_updated || was_paused || resolved.bootstrap_binding.is_some() {
            self.session_store.save(seed_session.clone()).await;
        }
        drop(initialization_guard);

        if is_new_session && hooks_enabled {
            let hook_point =
                session_lifecycle_hook_point(&resolved.descriptor.agent_id.0, "created");
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

        let idle_session_id = request.session_id.clone();
        let idle_sender_id = request.sender_id.clone();
        let idle_agent_id = resolved.descriptor.agent_id.0.clone();
        let idle_chain_depth = request.chain_depth;

        let handle = self.get_or_create_session_handle(seed_session).await;
        let mut turn_result = handle
            .run_turn(
                request,
                resolved,
                event_sink,
                interaction_handle,
                channel_file_sender,
            )
            .await;

        // Fire the session.lifecycle.state event hook after any non-error
        // turn termination. `run_turn` returns `Ok(AppTurnResult)` for ALL
        // four `AgentOutcome` variants (Complete / MaxTurnsReached /
        // BudgetExhausted / Cancelled) — they all leave the session back in
        // `idle` (ready for the next turn), so `state="idle"` is correct for
        // each. Only `Err(_)` (a true failure) is excluded; that branch
        // currently emits no event. The per-variant terminal kind is carried
        // in the payload's `outcome` field so plugins can distinguish a
        // normal completion from a soft termination without switching on
        // `state`.
        //
        // Actions requested by the hookers are collected into
        // `AppTurnResult.hook_actions`. Before they are returned, `SendPrompt`
        // entries are stamped with `chain_depth = idle_chain_depth + 1` and
        // dropped if that value **reaches** `max_prompt_chain_depth`
        // (`next_depth >= max`, an exclusive upper bound — the chain may run
        // `max_prompt_chain_depth` turns total: depth `0` (user-initiated)
        // plus `max - 1` `send_prompt`-triggered turns). Daemon-side execution
        // (e.g. `open_session` for `CreateSession`/`SwitchSession`/`SendPrompt`)
        // is performed by the HTTP router via its own `DaemonHookActionSink`
        // after `run_turn` returns, before forwarding to the TUI via the SSE
        // `Done` event. The trade-off vs a fire-and-forget design is that the
        // user sees the turn result after the hookers finish; acceptable
        // because session lifecycle hooks only fire after turn termination
        // anyway, and most setups register zero or fast hookers.
        if hooks_enabled {
            if let Ok(turn) = turn_result.as_mut() {
                let actions = self
                    .fire_session_state_hook_and_collect_actions(
                        idle_session_id,
                        idle_sender_id,
                        idle_agent_id,
                        "idle".to_string(),
                        turn.outcome.as_tag().to_string(),
                        idle_chain_depth,
                    )
                    .await;
                turn.hook_actions = actions;
            }
        }

        turn_result
    }
}

#[async_trait]
impl SessionService for CoreBackedSessionService {
    async fn run_turn(
        &self,
        request: AppTurnRequest,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, None, None, None, None).await
    }

    async fn run_turn_with_events(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(request, event_sink, None, None, None)
            .await
    }

    async fn run_turn_with_interaction(
        &self,
        request: AppTurnRequest,
        event_sink: Option<Arc<dyn LoopEventSink>>,
        interaction_handle: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender: Option<Arc<dyn ChannelFileSender>>,
        cancellation_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        self.run_turn_inner(
            request,
            event_sink,
            interaction_handle,
            channel_file_sender,
            cancellation_token,
        )
        .await
    }
}

#[async_trait]
impl SessionControlPlane for CoreBackedSessionService {
    async fn hibernate_idle_session(
        &self,
        session_id: &str,
        idle_before_ms: u64,
    ) -> Result<Option<SessionRecord>, SessionServiceError> {
        let Some(handle) = self.sessions_handler.lock().await.get(session_id).cloned() else {
            return Ok(None);
        };
        let Some(paused) = handle.hibernate_idle(idle_before_ms).await? else {
            return Ok(None);
        };
        self.sessions_handler.lock().await.remove(session_id);
        self.backend_manager
            .release_session(session_id)
            .await
            .map_err(|error| SessionServiceError::RuntimeShutdown {
                message: format!("failed to release hibernated local backend: {error}"),
            })?;
        Ok(Some(paused))
    }

    async fn open_session(
        &self,
        request: SessionOpenRequest,
    ) -> Result<SessionRecord, SessionServiceError> {
        let initialization_lock = self.runtime_initialization_lock(&request.session_id).await;
        let _initialization_guard = initialization_lock.lock().await;
        let existing_record = self.session_store.load(&request.session_id).await;
        let is_new_session = existing_record.is_none();
        let runtime_input = SessionRuntimeBuildInput::from_open_request(&request);
        // Resolve before every fast return so E2B binding conflicts cannot be
        // bypassed by an existing handle or a paused runtime.
        let mut resolved = self
            .runtime_resolver
            .resolve(&runtime_input, existing_record.as_ref())
            .await?;
        if let Some(existing) = &existing_record {
            if existing.status == SessionLifecycleStatus::Paused {
                return Ok(existing.clone());
            }
        }
        // Reuse an already-running in-memory handle if present. We
        // intentionally do NOT fall back to the store here: doing so via
        // `handle_for_session` would silently create a handle from a
        // stale/imported record (e.g. from `/load`) without leasing a
        // backend or running the state-preservation logic below, which
        // loses the LLM's context on the next turn. Records without a live
        // handle fall through to the build path, which properly preserves
        // imported state and leases a backend.
        if let Some(handle) = self
            .sessions_handler
            .lock()
            .await
            .get(&request.session_id)
            .cloned()
        {
            return handle.snapshot().await;
        }

        let mut session = Self::build_session_for_open(&request, &resolved);
        // Preserve state from any pre-existing store record (e.g. imported
        // via `/load`). Without this, `build_session_for_open` would
        // initialise `loop_state`/`memory_snapshot` to `None` and the
        // subsequent `session_store.save` below would overwrite the imported
        // record — silently erasing the LLM's message history and leaving
        // the model with no prior context even though the TUI still echoes
        // the old chat messages.
        if let Some(existing) = existing_record {
            session.loop_state = existing.loop_state;
            session.memory_snapshot = existing.memory_snapshot;
            session.agents = existing.agents;
            session.subagent_state = existing.subagent_state;
            session.parent_runtime_id = existing.parent_runtime_id;
            session.forked_from_checkpoint_id = existing.forked_from_checkpoint_id;
            session.created_at_ms = existing.created_at_ms;
        }
        let backend_lease = lease_session_backend(
            self.backend_manager.as_ref(),
            &session,
            &resolved,
            self.session_store.clone(),
        )
        .await?;
        if let Err(error) =
            crate::gateway::finalize_e2b_runtime(&mut resolved, backend_lease.backend()).await
        {
            if is_new_session {
                self.backend_manager
                    .release_session(&request.session_id)
                    .await
                    .ok();
            }
            return Err(error);
        }
        session.runtime.system_prompt = resolved.descriptor.system_prompt.clone();
        session.runtime.workspace_root = resolved.descriptor.workspace_root.clone();
        session.runtime.bootstrap_binding = resolved.bootstrap_binding.clone();
        if sync_session_backend_instance(&mut session, &backend_lease) {
            session.updated_at_ms = current_time_ms();
        }
        self.session_store.save(session.clone()).await;

        let hook_point = session_lifecycle_hook_point(&resolved.descriptor.agent_id.0, "created");
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
        // 1. Collect direct children BEFORE deleting this session's store
        //    entry. Children are sessions forked via `checkout` whose
        //    `parent_runtime_id` equals this session. We cascade-close them
        //    so that descendant sandboxes and snapshots are reclaimed
        //    together with the parent. Without this, forked runtimes would
        //    leak their backends and e2b sandboxes indefinitely.
        let children = self.session_store.list_children(session_id).await;

        // 2. Recursively close each child first (bottom-up). A child failure
        //    does not abort the parent's close; we log and continue so a
        //    single bad child cannot strand the whole subtree.
        for child in &children {
            if let Err(error) = self.force_close_session(&child.session_id).await {
                tracing::warn!(
                    session_id = %child.session_id,
                    parent_session_id = %session_id,
                    error = %error,
                    "cascade close of child runtime failed; continuing with parent close"
                );
            }
        }

        // 3. Close this session's handle (mark Closed) or fall back to the
        //    store-only path when no live handle exists.
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

        // 4. Delete provider snapshots (e.g. e2b snapshots) for every
        //    checkpoint created by this runtime, then drop the in-memory
        //    checkpoint records. This prevents remote snapshot leaks when a
        //    runtime is closed without an explicit `delete_checkpoint_snapshot`
        //    call. Failures are logged but do not abort the close: local
        //    tracking is still cleared so it does not accumulate.
        let checkpoints = self
            .runtime_checkpoints
            .list_checkpoints_for_runtime(session_id)
            .await;
        for checkpoint in &checkpoints {
            if let Some(backend_checkpoint) = checkpoint.backend_checkpoint.as_ref() {
                if backend_checkpoint.provider_snapshot_id.is_some() {
                    if let Err(error) = self
                        .backend_manager
                        .delete_checkpoint_snapshot(BackendCheckpointSnapshotDeleteRequest {
                            checkpoint: backend_checkpoint.clone(),
                        })
                        .await
                    {
                        tracing::warn!(
                            checkpoint_id = %checkpoint.checkpoint_id,
                            session_id = %session_id,
                            error = %error,
                            "failed to delete checkpoint snapshot during close; snapshot may linger remotely"
                        );
                    }
                }
            }
        }
        self.runtime_checkpoints.remove_runtime(session_id).await;

        if !was_already_closed {
            let hook_point = session_lifecycle_hook_point(&closed.runtime.agent_id.0, "closed");
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
        self.checkpoint_runtime_internal(request).await
    }

    async fn checkout_runtime(
        &self,
        request: RuntimeCheckoutRequest,
    ) -> Result<RuntimeCheckoutResult, SessionServiceError> {
        self.checkout_runtime_internal(request).await
    }

    async fn pause_runtime(
        &self,
        request: RuntimePauseRequest,
    ) -> Result<RuntimePauseResult, SessionServiceError> {
        self.pause_runtime_internal(request).await
    }

    async fn resume_runtime(
        &self,
        request: RuntimeResumeRequest,
    ) -> Result<RuntimeResumeResult, SessionServiceError> {
        self.resume_runtime_internal(request).await
    }

    async fn delete_checkpoint_snapshot(
        &self,
        request: RuntimeCheckpointSnapshotDeleteRequest,
    ) -> Result<RuntimeCheckpointSnapshotDeleteResult, SessionServiceError> {
        self.delete_checkpoint_snapshot_internal(request).await
    }

    async fn exec_runtime(
        &self,
        request: RuntimeExecRequest,
    ) -> Result<RuntimeExecResult, SessionServiceError> {
        let lease = self
            .lease_bound_backend_for_idle_runtime(&request.runtime_id)
            .await?;
        let env = (!request.env.is_empty()).then(|| request.env.into_iter().collect());
        let backend = lease.backend();
        let shell = resolve_runtime_exec_shell(request.shell, backend.exec().default_shell());
        let result = backend
            .exec()
            .exec(ExecRequest {
                command: request.command,
                args: Vec::new(),
                shell: Some(shell),
                cwd: request.cwd.map(BackendPath),
                timeout_ms: request.timeout_ms,
                env,
            })
            .await;
        let result = match result {
            Ok(result) => result,
            Err(agent_contracts::backend::OperationError::ExecutionInterrupted {
                message,
                stdout,
                stderr,
                state,
            }) => {
                return Err(SessionServiceError::RuntimeExecInterrupted {
                    message: format!("runtime exec failed: {message}"),
                    stdout_base64: base64::engine::general_purpose::STANDARD.encode(stdout),
                    stderr_base64: base64::engine::general_purpose::STANDARD.encode(stderr),
                    execution_state: state,
                });
            }
            Err(error) => {
                return Err(SessionServiceError::CoreRun {
                    message: format!("runtime exec failed: {error}"),
                });
            }
        };

        Ok(RuntimeExecResult {
            stdout_base64: base64::engine::general_purpose::STANDARD.encode(result.stdout),
            stderr_base64: base64::engine::general_purpose::STANDARD.encode(result.stderr),
            exit_code: result.exit_code,
            timed_out: result.timed_out,
        })
    }

    async fn read_runtime_file(
        &self,
        request: RuntimeReadFileRequest,
    ) -> Result<RuntimeReadFileResult, SessionServiceError> {
        let lease = self
            .lease_bound_backend_for_idle_runtime(&request.runtime_id)
            .await?;
        let content = lease
            .backend()
            .files()
            .read_bytes(ReadBytesRequest {
                path: BackendPath(request.path),
            })
            .await
            .map_err(|error| SessionServiceError::CoreRun {
                message: format!("runtime file read failed: {error}"),
            })?;

        Ok(RuntimeReadFileResult {
            content_base64: base64::engine::general_purpose::STANDARD.encode(content),
        })
    }

    async fn write_runtime_file(
        &self,
        request: RuntimeWriteFileRequest,
    ) -> Result<RuntimeWriteFileResult, SessionServiceError> {
        let lease = self
            .lease_bound_backend_for_idle_runtime(&request.runtime_id)
            .await?;
        let content = base64::engine::general_purpose::STANDARD
            .decode(request.content_base64)
            .map_err(|error| SessionServiceError::RuntimeBuild {
                message: format!("invalid content_base64: {error}"),
            })?;
        let outcome = lease
            .backend()
            .files()
            .write_bytes(WriteBytesRequest {
                path: BackendPath(request.path),
                content,
                mode: WriteMode::Overwrite,
            })
            .await
            .map_err(|error| SessionServiceError::CoreRun {
                message: format!("runtime file write failed: {error}"),
            })?;

        Ok(RuntimeWriteFileResult {
            path: outcome.path.0,
            created: outcome.created,
        })
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
        match value {
            SessionRuntimeResolveError::InvalidBootstrap { message } => {
                Self::InvalidRequest { message }
            }
            SessionRuntimeResolveError::BootstrapConflict { message } => {
                Self::RuntimeConflict { message }
            }
            SessionRuntimeResolveError::BootstrapTooLarge { message } => {
                Self::PayloadTooLarge { message }
            }
            other => Self::RuntimeResolve {
                message: other.to_string(),
            },
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

/// Build a `*.Session.lifecycle.<stage>` hook point id. Consolidates the
/// inline `format!("{}.Session.lifecycle.<stage>", agent_id)` previously
/// repeated across the session created/closed/state call sites.
fn session_lifecycle_hook_point(agent_id: &str, stage: &str) -> HookPointId {
    HookPointId(format!("{}.Session.lifecycle.{}", agent_id, stage))
}

/// Stamp each `SendPrompt` action with `chain_depth = emitting_turn_depth
/// + 1` (overwriting any plugin-supplied value) and drop it once the
/// stamped value **reaches** `max_depth` (i.e., `next_depth >= max_depth`
/// — exclusive upper bound). Other action kinds pass through unchanged.
/// Pure (no I/O) so it can be unit-tested directly.
///
/// Semantics: `max_depth = N` permits a chain of **N turns total** — the
/// user-initiated turn at depth `0` plus `N - 1` `send_prompt`-triggered
/// turns at depths `1..=N-1`. A `send_prompt` that would start a turn at
/// depth `N` is dropped, so the chain stops after the depth-`N-1` turn.
/// The cap is the cross-turn `send_prompt` chain depth limit (default
/// `DEFAULT_MAX_PROMPT_CHAIN_DEPTH` = 128, configurable via
/// `[hooker].max_prompt_chain_depth`).
///
/// Called by [`CoreBackedSessionService::fire_session_state_hook_and_collect_actions`]
/// after the hookers return, before the actions are forwarded to the TUI.
/// The stamped `chain_depth` rides along on the forwarded action so the
/// TUI can relay it back via `RuntimeTurnRequest.chain_depth`, letting the
/// daemon track the resulting turn's depth and re-enforce the cap when
/// that turn ends.
fn stamp_and_cap_send_prompt_actions(
    actions: Vec<HookAction>,
    emitting_turn_depth: usize,
    max_depth: usize,
) -> Vec<HookAction> {
    actions
        .into_iter()
        .filter_map(|action| match action {
            HookAction::SendPrompt {
                session_id, text, ..
            } => {
                let next_depth = emitting_turn_depth.saturating_add(1);
                if next_depth >= max_depth {
                    tracing::warn!(
                        session_id = %session_id,
                        next_depth,
                        max_depth,
                        "send_prompt hook action dropped: chain depth reaches cap"
                    );
                    return None;
                }
                Some(HookAction::SendPrompt {
                    session_id,
                    text,
                    chain_depth: next_depth,
                })
            }
            other => Some(other),
        })
        .collect()
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
    use xiaoo_core::LoopStateSnapshot;

    #[test]
    fn runtime_exec_shell_prefers_explicit_shell() {
        assert_eq!(
            resolve_runtime_exec_shell(Some("/bin/zsh".to_string()), Some("/bin/bash")),
            "/bin/zsh"
        );
    }

    #[test]
    fn runtime_exec_shell_uses_backend_default() {
        assert_eq!(
            resolve_runtime_exec_shell(None, Some("/bin/bash")),
            "/bin/bash"
        );
    }

    #[test]
    fn runtime_exec_shell_falls_back_to_posix_shell() {
        assert_eq!(resolve_runtime_exec_shell(None, None), "/bin/sh");
    }

    #[test]
    fn stamp_and_cap_stamps_send_prompt_with_next_depth() {
        // A plugin omits `chain_depth` (parses to 0); the daemon overwrites
        // it with `emitting_turn_depth + 1` regardless of the plugin value.
        let actions = vec![HookAction::SendPrompt {
            session_id: "s1".into(),
            text: "hi".into(),
            chain_depth: 0,
        }];
        let stamped = stamp_and_cap_send_prompt_actions(actions, 5, 128);
        assert_eq!(stamped.len(), 1);
        match &stamped[0] {
            HookAction::SendPrompt { chain_depth, .. } => assert_eq!(*chain_depth, 6),
            _ => panic!("expected SendPrompt"),
        }
    }

    #[test]
    fn stamp_and_cap_overwrites_plugin_supplied_depth() {
        // A plugin cannot forge a low depth to bypass the cap: the daemon
        // ignores any plugin-supplied value and stamps the real next depth.
        // Here `next_depth = 129 >= 128` → dropped regardless of the plugin
        // value (emitting depth 128 is itself only reachable if the cap were
        // higher; the point of this test is the override + drop, not the
        // reachability of the emitting depth).
        let actions = vec![HookAction::SendPrompt {
            session_id: "s1".into(),
            text: "hi".into(),
            chain_depth: 0, // plugin tries to look like a depth-0 turn
        }];
        let stamped = stamp_and_cap_send_prompt_actions(actions, 128, 128);
        // next = 129 >= 128 cap → dropped, not forwarded.
        assert!(stamped.is_empty());
    }

    #[test]
    fn stamp_and_cap_drops_send_prompt_above_cap() {
        let actions = vec![HookAction::SendPrompt {
            session_id: "s1".into(),
            text: "hi".into(),
            chain_depth: 0,
        }];
        // emitting depth = cap (3) → next = 4 >= 3 → dropped (way above cap).
        assert!(stamp_and_cap_send_prompt_actions(actions, 3, 3).is_empty());
    }

    #[test]
    fn stamp_and_cap_drops_send_prompt_at_boundary() {
        // Cap is an EXCLUSIVE upper bound: a `send_prompt` whose next-turn
        // depth would equal `max_depth` is dropped. `max_depth = N` permits
        // turns at depths `0..=N-1` (N turns total); the `N`-th-turn-after-
        // user (depth `N`) must not start.
        //
        // emitting depth = cap - 1 = 2 → next = 3 == cap → dropped.
        let actions = vec![HookAction::SendPrompt {
            session_id: "s1".into(),
            text: "hi".into(),
            chain_depth: 0,
        }];
        let stamped = stamp_and_cap_send_prompt_actions(actions, 2, 3);
        assert!(
            stamped.is_empty(),
            "next_depth == max_depth must be dropped (exclusive cap)"
        );
    }

    #[test]
    fn stamp_and_cap_keeps_send_prompt_just_below_boundary() {
        // emitting depth = cap - 2 = 1 → next = 2 = cap - 1 < cap → the
        // last allowed turn (depth `max - 1`) is permitted to run. With
        // max=3 this is the 3rd turn in the chain (depths 0, 1, 2).
        let actions = vec![HookAction::SendPrompt {
            session_id: "s1".into(),
            text: "hi".into(),
            chain_depth: 0,
        }];
        let stamped = stamp_and_cap_send_prompt_actions(actions, 1, 3);
        assert_eq!(stamped.len(), 1);
        match &stamped[0] {
            HookAction::SendPrompt { chain_depth, .. } => assert_eq!(*chain_depth, 2),
            _ => panic!("expected SendPrompt"),
        }
    }

    #[test]
    fn stamp_and_cap_passes_other_action_kinds_through_unchanged() {
        let actions = vec![
            HookAction::CreateSession {
                session_id: "a".into(),
            },
            HookAction::SwitchSession {
                session_id: "a".into(),
            },
        ];
        let stamped = stamp_and_cap_send_prompt_actions(actions, 999, 1);
        assert_eq!(stamped.len(), 2);
        assert!(matches!(stamped[0], HookAction::CreateSession { .. }));
        assert!(matches!(stamped[1], HookAction::SwitchSession { .. }));
    }

    #[test]
    fn stamp_and_cap_mixed_batch_keeps_non_send_and_caps_send() {
        let actions = vec![
            HookAction::CreateSession {
                session_id: "a".into(),
            },
            HookAction::SendPrompt {
                session_id: "a".into(),
                text: "x".into(),
                chain_depth: 0,
            },
            HookAction::SwitchSession {
                session_id: "a".into(),
            },
            HookAction::SendPrompt {
                session_id: "b".into(),
                text: "y".into(),
                chain_depth: 0,
            },
        ];
        // emitting depth 3, cap 3: first send_prompt → next 4 >= 3 dropped;
        // second send_prompt also dropped; create/switch pass through.
        let stamped = stamp_and_cap_send_prompt_actions(actions, 3, 3);
        assert_eq!(stamped.len(), 2);
        assert!(matches!(stamped[0], HookAction::CreateSession { .. }));
        assert!(matches!(stamped[1], HookAction::SwitchSession { .. }));
    }

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
            if request.workspace.as_deref() == Some(std::path::Path::new("/conflict")) {
                return Err(SessionRuntimeResolveError::BootstrapConflict {
                    message: "test conflict".to_string(),
                });
            }
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
                backend_workspace_root: self.workspace_root.clone(),
                e2b_bootstrap: None,
                bootstrap_binding: None,
                e2b_finalized: false,
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
            workspace: None,
            skills: None,
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
                workspace: None,
                skills: None,
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
    async fn existing_handle_does_not_bypass_bootstrap_binding_validation() {
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
            .open_session(test_open_request("binding-fast-path"))
            .await
            .expect("initial open");
        let mut conflicting = test_open_request("binding-fast-path");
        conflicting.workspace = Some(std::path::PathBuf::from("/conflict"));
        let error = dependencies
            .session_control_plane
            .open_session(conflicting)
            .await
            .expect_err("binding conflict should be checked before handle reuse");
        assert!(matches!(error, SessionServiceError::RuntimeConflict { .. }));
    }

    #[tokio::test]
    async fn open_session_preserves_imported_loop_state() {
        // Regression: `/load` imports a SessionRecord (with loop_state
        // containing the LLM's prior message history) into the in-memory
        // store, then the next turn calls `ensure_session_open` ->
        // `open_session`. Before the fix, `open_session` built a fresh
        // SessionRecord via `build_session_for_open` (which sets
        // `loop_state: None`) and overwrote the imported record, so the
        // LLM lost all context even though the TUI still echoed the old
        // chat messages.
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver.clone(),
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        // Simulate `/load`: seed the store with an Idle record carrying a
        // non-empty loop_state (two ChatMessages).
        let imported_messages = vec![
            agent_types::ChatMessage {
                role: agent_types::MessageRole::User,
                blocks: vec![agent_types::ContentBlock::Text {
                    text: "earlier user prompt".to_string(),
                }],
                message_id: None,
                timestamp_ms: 0,
                api_usage_tokens: None,
                reasoning_content: None,
                estimated_tokens: None,
            },
            agent_types::ChatMessage {
                role: agent_types::MessageRole::Assistant,
                blocks: vec![agent_types::ContentBlock::Text {
                    text: "earlier assistant reply".to_string(),
                }],
                message_id: None,
                timestamp_ms: 0,
                api_usage_tokens: None,
                reasoning_content: None,
                estimated_tokens: None,
            },
        ];
        let mut seeded = save_session_without_backend(
            &store,
            &resolver,
            "s-import",
            SessionLifecycleStatus::Idle,
        )
        .await;
        seeded.loop_state = Some(LoopStateSnapshot {
            session_id: "s-import".to_string(),
            messages: imported_messages.clone(),
            turn_count: 1,
            token_usage: Default::default(),
            compression_meta: Default::default(),
            kv_cache_map: Default::default(),
        });
        store.save(seeded.clone()).await;

        // Now the user sends a new prompt -> `open_session` runs.
        let opened = dependencies
            .session_control_plane
            .open_session(SessionOpenRequest {
                session_id: "s-import".to_string(),
                conversation_id: "c-import".to_string(),
                sender_id: "u1".to_string(),
                entry: GatewayEntryContext::tui(None),
                channel: None,
                channel_instance_id: None,
                llm: None,
                workspace: None,
                skills: None,
            })
            .await
            .expect("open imported session");

        let retained = opened
            .loop_state
            .as_ref()
            .expect("loop_state must be preserved across open_session");
        assert_eq!(retained.messages, imported_messages);
        assert_eq!(retained.turn_count, 1);

        // The build path (not the `handle_for_session` shortcut) must have
        // run: a backend instance is leased and persisted, proving the
        // state-preservation block above was actually executed.
        let leased_instance = opened
            .backend_instance
            .as_ref()
            .expect("open_session must lease a backend for imported records");
        assert_eq!(leased_instance.session_id, "s-import");
        assert_eq!(leased_instance.state, BackendLifecycleState::Active);

        // And the store must keep the preserved state, not a wiped record.
        let stored = store
            .load("s-import")
            .await
            .expect("store must retain record");
        assert_eq!(
            stored
                .loop_state
                .as_ref()
                .expect("stored loop_state must be preserved")
                .messages,
            imported_messages
        );
        let stored_instance = stored
            .backend_instance
            .as_ref()
            .expect("store must retain leased backend instance");
        assert_eq!(stored_instance.backend_id, leased_instance.backend_id);
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
                workspace: None,
                skills: None,
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
                workspace: None,
                skills: None,
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
    async fn pause_runtime_releases_backend_and_marks_runtime_paused() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let backend_manager = Arc::new(BackendManager::new());
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver,
            HookerRegistryConfig::default(),
            backend_manager.clone(),
        )
        .expect("dependencies");

        dependencies
            .session_control_plane
            .open_session(test_open_request("runtime-pause"))
            .await
            .expect("open session");

        let paused = dependencies
            .session_control_plane
            .pause_runtime(RuntimePauseRequest {
                runtime_id: "runtime-pause".to_string(),
                metadata: Value::Null,
                name: Some("pause".to_string()),
            })
            .await
            .expect("pause runtime");

        assert_eq!(paused.runtime.runtime_id, "runtime-pause");
        assert_eq!(paused.runtime.status, SessionLifecycleStatus::Paused);
        let saved = store.load("runtime-pause").await.expect("saved runtime");
        assert_eq!(saved.status, SessionLifecycleStatus::Paused);
        assert!(saved.backend_instance.is_none());
        assert!(matches!(
            backend_manager.lease_bound_session("runtime-pause").await,
            Err(BackendError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn resume_runtime_reuses_same_runtime_id_for_paused_runtime_without_backend() {
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
            "runtime-resume",
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

        dependencies
            .session_control_plane
            .pause_runtime(RuntimePauseRequest {
                runtime_id: "runtime-resume".to_string(),
                metadata: Value::Null,
                name: None,
            })
            .await
            .expect("pause runtime");
        let resumed = dependencies
            .session_control_plane
            .resume_runtime(RuntimeResumeRequest {
                runtime_id: "runtime-resume".to_string(),
                metadata: Value::Null,
            })
            .await
            .expect("resume runtime");

        assert_eq!(resumed.runtime.runtime_id, "runtime-resume");
        assert_eq!(resumed.runtime.status, SessionLifecycleStatus::Idle);
        let saved = store.load("runtime-resume").await.expect("saved runtime");
        assert_eq!(saved.status, SessionLifecycleStatus::Idle);
        assert!(saved.backend_instance.is_none());
    }

    #[tokio::test]
    async fn close_paused_runtime_removes_session_record() {
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
            "runtime-close-paused",
            SessionLifecycleStatus::Paused,
        )
        .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver,
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        let closed = dependencies
            .session_control_plane
            .force_close_session("runtime-close-paused")
            .await
            .expect("close paused runtime");

        assert_eq!(closed.status, SessionLifecycleStatus::Closed);
        assert!(store.load("runtime-close-paused").await.is_none());
    }

    #[tokio::test]
    async fn run_turn_paused_runtime_without_checkpoint_is_busy() {
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
            "runtime-paused-turn",
            SessionLifecycleStatus::Paused,
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
            .session_service
            .run_turn(test_open_request("runtime-paused-turn").into_turn_request("hi".to_string()))
            .await;

        // A paused session without an eviction checkpoint cannot be resumed
        // and must be reported as busy rather than silently rejected.
        assert!(matches!(
            result,
            Err(SessionServiceError::SessionBusy { message, .. })
                if message.contains("no eviction checkpoint")
        ));
    }

    #[tokio::test]
    async fn hibernated_mcp_runtime_keeps_record_and_rehydrates_local_backend() {
        let workspace = TempDir::new().expect("workspace");
        let store = Arc::new(InMemorySessionStore::default());
        let resolver = Arc::new(StubRuntimeResolver {
            workspace_root: workspace.path().to_path_buf(),
            backend_options: json!({"temp_root": workspace.path().to_string_lossy().to_string()}),
            llm_provider: stub_llm_provider(),
        });
        let backend_manager = Arc::new(BackendManager::new());
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver,
            HookerRegistryConfig::default(),
            backend_manager.clone(),
        )
        .expect("dependencies");
        let mut open = test_open_request("mcp-hibernate");
        open.entry = GatewayEntryContext {
            kind: Some(crate::gateway::GatewayEntryKind::Mcp),
            instance_id: Some("chatbot".to_string()),
            runtime_profile_id: None,
            build_tags: Vec::new(),
        };
        dependencies
            .session_control_plane
            .open_session(open.clone())
            .await
            .expect("open MCP session");

        let paused = dependencies
            .session_control_plane
            .hibernate_idle_session("mcp-hibernate", u64::MAX)
            .await
            .expect("hibernate")
            .expect("idle session should hibernate");
        assert_eq!(paused.status, SessionLifecycleStatus::Paused);
        assert!(paused.backend_instance.is_none());
        let saved = store.load("mcp-hibernate").await.expect("record retained");
        assert_eq!(saved.status, SessionLifecycleStatus::Paused);
        assert!(matches!(
            backend_manager.lease_bound_session("mcp-hibernate").await,
            Err(BackendError::NotFound { .. })
        ));

        let result = dependencies
            .session_service
            .run_turn(open.into_turn_request("continue".to_string()))
            .await;
        assert!(
            !matches!(
                result,
                Err(SessionServiceError::SessionBusy { ref message, .. })
                    if message.contains("no eviction checkpoint")
            ),
            "hibernated MCP local sessions must rebuild instead of requiring a checkpoint"
        );
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

    #[tokio::test]
    async fn force_close_session_cascades_to_descendants() {
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
            "rt-parent",
            SessionLifecycleStatus::Idle,
        )
        .await;
        let dependencies = AppBootstrap::from_session_components_with_hooks_and_backend_manager(
            store.clone(),
            resolver.clone(),
            HookerRegistryConfig::default(),
            Arc::new(BackendManager::new()),
        )
        .expect("dependencies");

        // parent -> checkpoint -> child
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
        let child_id = checkout.runtime.runtime_id.clone();
        assert!(store.load(&child_id).await.is_some());

        // Manually register a grandchild whose parent_runtime_id points at
        // the child, to exercise recursive cascade without a second
        // checkpoint/checkout round-trip.
        let mut grandchild = save_session_without_backend(
            &store,
            &resolver,
            "rt-grandchild",
            SessionLifecycleStatus::Idle,
        )
        .await;
        grandchild.parent_runtime_id = Some(child_id.clone());
        store.save(grandchild.clone()).await;
        assert!(store.load("rt-grandchild").await.is_some());

        // Closing the parent must cascade-close child and grandchild.
        dependencies
            .session_control_plane
            .force_close_session(&parent.session_id)
            .await
            .expect("force close parent");

        assert!(
            store.load("rt-parent").await.is_none(),
            "parent should be deleted"
        );
        assert!(
            store.load(&child_id).await.is_none(),
            "child should be cascade-deleted"
        );
        assert!(
            store.load("rt-grandchild").await.is_none(),
            "grandchild should be cascade-deleted"
        );

        // The checkpoint record for the parent must also be gone, so a
        // subsequent checkout from it fails with SessionNotFound.
        let result = dependencies
            .session_control_plane
            .checkout_runtime(RuntimeCheckoutRequest {
                checkpoint_id: checkpoint.checkpoint_id.clone(),
                conversation_id: None,
                sender_id: None,
                metadata: Value::Null,
            })
            .await;
        assert!(
            matches!(result, Err(SessionServiceError::SessionNotFound { .. })),
            "checkpoint should have been removed during close, got: {:?}",
            result
        );
    }
}
