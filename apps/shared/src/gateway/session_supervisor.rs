use crate::backend::BackendManager;
use crate::gateway::session_backend::{lease_session_backend, sync_session_backend_instance};
use crate::gateway::session_record::SessionAgentRecord;
use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionLifecycleStatus, SessionRecord,
    SessionRuntimeBuildInput, SessionRuntimeResolver, SessionServiceError, SessionStore,
    TurnOutcome,
};
use agent_contracts::backend::OperationBackend;
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink, ToolEventSink};
use agent_types::common::ids::AgentId;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use agent_types::outcome::AgentOutcome;
use agent_types::tool::{RawToolOutcome, ToolExecutionError, ToolExecutionResult};
use agent_types::ReasoningEffort;
use memory::MemorySnapshot;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use subagent::{
    HostAction, JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControlError, SubagentCoordinator, SubagentTerminalKind, SubagentTerminalSnapshot,
};
use tokio::sync::{oneshot, Mutex};
use tokio_util::sync::CancellationToken;
use tool::ToolSpecSnapshot;
use xiaoo_api::runtime::build_tool_result_message;
use xiaoo_api::runtime::{LoopStateSnapshot, LoopSuspendReason, RuntimeOutput, SuspendedToolCall};

use super::session_worker::{SessionWorker, SessionWorkerInput};

struct PendingJoinWaiter {
    sender: Option<oneshot::Sender<SubagentTerminalSnapshot>>,
    receiver: Option<oneshot::Receiver<SubagentTerminalSnapshot>>,
}

struct PendingInteractionWaiter {
    agent_id: AgentId,
    response_tx: oneshot::Sender<InteractionResponse>,
}

/// Root turn's sinks, inherited by spawned subagent lanes in remote
/// (daemon) mode where the resolver returns default bindings. Set in
/// `run_root_turn`, read in `run_lane_until_terminal`, cleared on exit.
struct RootTurnSinks {
    loop_event_sink: Option<Arc<dyn LoopEventSink>>,
    tool_event_sink: Option<Arc<dyn ToolEventSink>>,
    /// The parent (root) turn's real interaction handle. In remote/daemon
    /// mode the resolver returns default (None) bindings, so a spawned
    /// subagent's `ask_user_question` cannot find a handle via the resolver.
    /// The per-turn handle (e.g. `RemoteSseInteractionHandle`) is only stored
    /// here so `request_interaction` can forward subagent questions to the
    /// user through the same channel/SSE the root turn uses.
    interaction_handle: Option<Arc<dyn InteractionHandle>>,
}

struct LaneRunInput {
    agent_id: AgentId,
    runtime_input: SessionRuntimeBuildInput,
    resolved_runtime: Option<ResolvedSessionRuntime>,
    user_message: String,
    append_user_message: bool,
    reasoning_effort: ReasoningEffort,
    loop_event_sink_override: Option<Arc<dyn LoopEventSink>>,
    interaction_handle_override: Option<Arc<dyn InteractionHandle>>,
    channel_file_sender_override: Option<Arc<dyn ChannelFileSender>>,
    cancellation_token: Option<CancellationToken>,
    command_context: Option<agent_types::chat::CommandContext>,
}

struct LaneTerminal {
    result: AppTurnResult,
    terminal: SubagentTerminalSnapshot,
    loop_state: LoopStateSnapshot,
    memory_snapshot: MemorySnapshot,
}

pub(crate) struct SessionSupervisor {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    backend_manager: Arc<BackendManager>,
    coordinator: SubagentCoordinator,
    session: Mutex<SessionRecord>,
    pending_joins: Mutex<HashMap<String, PendingJoinWaiter>>,
    pending_interactions: Mutex<HashMap<String, PendingInteractionWaiter>>,
    interaction_semaphore: Arc<tokio::sync::Semaphore>,
    root_turn_lock: Mutex<()>,
    /// Root turn's sinks, inherited by spawned subagent lanes in remote
    /// (daemon) mode where the resolver returns default bindings. Set
    /// in `run_root_turn`, read in `run_lane_until_terminal`, cleared
    /// on exit. Kept as a single `Mutex<Option<RootTurnSinks>>` so the
    /// pair is always set/cleared atomically.
    current_root_sinks: Mutex<Option<RootTurnSinks>>,
    /// Optional cap on how long a forwarded subagent interaction
    /// (`ask_user_question`) may wait for the user to reply. Only armed
    /// when the handle does not enforce its own timeout (see
    /// [`InteractionHandle::has_builtin_timeout`]); see
    /// `request_interaction` for why self-timing handles skip it.
    /// `None` = no outer cap.
    interaction_timeout: Option<Duration>,
}

impl SessionSupervisor {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        backend_manager: Arc<BackendManager>,
        session: SessionRecord,
        interaction_timeout: Option<Duration>,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            backend_manager,
            coordinator: SubagentCoordinator::new(),
            session: Mutex::new(session),
            pending_joins: Mutex::new(HashMap::new()),
            pending_interactions: Mutex::new(HashMap::new()),
            interaction_semaphore: Arc::new(tokio::sync::Semaphore::new(1)),
            root_turn_lock: Mutex::new(()),
            current_root_sinks: Mutex::new(None),
            interaction_timeout,
        }
    }

    pub async fn snapshot(&self) -> SessionRecord {
        self.session.lock().await.clone()
    }

    pub(crate) async fn prepare_root_turn(
        &self,
        request: &AppTurnRequest,
        resolved: &ResolvedSessionRuntime,
    ) {
        let mut session = self.session.lock().await;
        session.conversation_id = request.conversation_id.clone();
        session.sender_id = request.sender_id.clone();
        session.entry = request.entry.clone();
        session.channel = request.channel.clone();
        session.channel_instance_id = request.channel_instance_id.clone();
        session.runtime.agent_id = resolved.descriptor.agent_id.clone();
        session.runtime.model = resolved.descriptor.model.clone();
        session.runtime.llm = resolved.descriptor.llm.clone();
        session.runtime.system_prompt = resolved.descriptor.system_prompt.clone();
        session.runtime.feature_flags = resolved.descriptor.feature_flags.clone();
        session.runtime.token_budget = resolved.descriptor.token_budget.clone();
        session.runtime.workspace_root = resolved.descriptor.workspace_root.clone();
        session.runtime.max_turns = resolved.descriptor.max_turns;
        session.updated_at_ms = current_time_ms();
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot).await;
    }

    pub(crate) async fn force_close(&self) -> SessionRecord {
        let session_id;
        let snapshot = {
            let mut session = self.session.lock().await;
            session.status = SessionLifecycleStatus::Closed;
            session.updated_at_ms = current_time_ms();
            session_id = session.session_id.clone();
            let snapshot = session.clone();
            drop(session);
            self.session_store.save(snapshot.clone()).await;
            snapshot
        };
        // Report the closed status to the shared registry so the backend
        // can be evicted promptly instead of staying stuck as "running".
        self.report_session_status(&session_id, &SessionLifecycleStatus::Closed)
            .await;
        snapshot
    }

    pub(crate) async fn hibernate_idle(&self) -> SessionRecord {
        let mut session = self.session.lock().await;
        session.status = SessionLifecycleStatus::Paused;
        session.backend_instance = None;
        session.paused_backend_checkpoint = None;
        session.last_error = None;
        session.updated_at_ms = current_time_ms();
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot.clone()).await;
        snapshot
    }

    pub(crate) async fn request_interaction(
        self: &Arc<Self>,
        request_id: String,
        agent_id: AgentId,
        parent_agent_id: AgentId,
        request: InteractionRequest,
        response_tx: oneshot::Sender<InteractionResponse>,
    ) {
        let supervisor = Arc::clone(self);
        let waiter = PendingInteractionWaiter {
            agent_id: agent_id.clone(),
            response_tx,
        };

        self.pending_interactions
            .lock()
            .await
            .insert(request_id.clone(), waiter);

        let session = self.session.lock().await.clone();
        // Prefer the current root turn's real interaction handle: in
        // remote/daemon mode the resolver returns default (None) bindings,
        // so `load_interaction_handle` yields None and the subagent's
        // question would hang forever. Fall back to the resolver only when
        // no root-turn handle is stored (local CLI/TUI / channel mode where
        // the handle lives in the resolver's static bindings).
        let interaction_handle = {
            let sinks = self.current_root_sinks.lock().await;
            sinks.as_ref().and_then(|s| s.interaction_handle.clone())
        };
        let interaction_handle = match interaction_handle {
            Some(handle) => Some(handle),
            None => {
                self.load_interaction_handle(&session, &parent_agent_id)
                    .await
            }
        };
        let semaphore = self.interaction_semaphore.clone();
        let interaction_timeout = self.interaction_timeout;
        let has_builtin_timeout = interaction_handle
            .as_ref()
            .is_some_and(|h| h.has_builtin_timeout());
        // Only arm the outer `select!` when the handle does not enforce
        // its own timeout AND a timeout is configured: racing a
        // self-timing handle would drop its `ask` future mid-cleanup,
        // leaking its pending entry and skipping the user-facing timeout
        // notice (see `ChannelInteractionHandle::has_builtin_timeout`).
        // Other handles get the outer cap and `abort_pending` on expiry.
        let outer_timeout = if has_builtin_timeout {
            None
        } else {
            interaction_timeout
        };

        if let Some(handle) = interaction_handle {
            tokio::spawn(async move {
                let _permit = semaphore.acquire().await.unwrap();
                let response = match outer_timeout {
                    Some(duration) => {
                        tokio::select! {
                            biased;
                            r = handle.ask(&request) => r,
                            _ = tokio::time::sleep(duration) => {
                                tracing::warn!(
                                    request_id = %request_id,
                                    timeout_secs = duration.as_secs(),
                                    "subagent interaction timed out before the user \
                                     replied; delivering sentinel response"
                                );
                                // Drop-then-abort: `handle.ask` future is
                                // cancelled by `select!` here. Any external
                                // pending state it registered (e.g. an SSE
                                // interaction store entry) is released via
                                // `abort_pending` so a late user reply is
                                // not silently swallowed by a stale entry.
                                handle.abort_pending(&request).await;
                                super::subagent_interaction::interaction_timeout_response(
                                    &request,
                                )
                            }
                        }
                    }
                    None => handle.ask(&request).await,
                };
                supervisor
                    .deliver_interaction_response_from_user(request_id, response)
                    .await;
            });
        } else {
            // No handle to route through (no root-turn sink AND the resolver
            // returned None): the spawned `ask` task would never run, so
            // the waiter we inserted into `pending_interactions` would
            // leak forever and the subagent lane would block on
            // `response_rx` indefinitely. Deliver a sentinel now so the
            // lane winds down and the waiter is reaped.
            tracing::warn!(
                request_id = %request_id,
                parent_agent_id = %parent_agent_id,
                "no interaction handle available for subagent request; \
                 delivering sentinel response"
            );
            let response = super::subagent_interaction::interaction_timeout_response(&request);
            supervisor
                .deliver_interaction_response_from_user(request_id, response)
                .await;
        }
    }

    async fn load_interaction_handle(
        &self,
        session: &SessionRecord,
        agent_id: &AgentId,
    ) -> Option<Arc<dyn InteractionHandle>> {
        let mut runtime_input = runtime_input_from_session(session, agent_id.clone(), None);
        runtime_input.agent_id_override = Some(agent_id.clone());

        let resolved = self
            .runtime_resolver
            .resolve(&runtime_input, Some(session))
            .await
            .ok()?;

        resolved.bindings.interaction_handle.clone()
    }

    pub(crate) async fn deliver_interaction_response_from_user(
        self: &Arc<Self>,
        request_id: String,
        response: InteractionResponse,
    ) {
        let waiter = self.pending_interactions.lock().await.remove(&request_id);
        if let Some(waiter) = waiter {
            if waiter.response_tx.send(response).is_err() {
                tracing::warn!(
                    request_id = %request_id,
                    agent_id = %waiter.agent_id,
                    "failed to deliver interaction response: receiver dropped"
                );
            }
        } else {
            tracing::warn!(
                request_id = %request_id,
                "interaction waiter not found for response delivery"
            );
        }
    }

    pub async fn spawn_subagent(
        self: &Arc<Self>,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError> {
        self.ensure_session_match(&request.session_id).await?;

        let child_agent_id = AgentId(uuid::Uuid::new_v4().to_string());
        let now_ms = current_time_ms();

        let mut request = request;
        if let Some(role_id) = &request.subagent_role_id {
            let session = self.session.lock().await;
            if let Some(role) = session.runtime.subagent_roles.get(role_id) {
                request.predefined_prompt = role.prompt.clone();
                request.max_turns = role.max_turns;
                request.description = if request.description.is_empty() {
                    role.description.clone()
                } else {
                    request.description.clone()
                };
            }
        }

        let mut session = self.session.lock().await;
        let decision = self.coordinator.spawn(
            &mut session.subagent_state,
            &request,
            child_agent_id.clone(),
            now_ms,
        )?;
        session.agents.insert(
            child_agent_id.0.clone(),
            SessionAgentRecord {
                agent_id: child_agent_id.clone(),
                parent_agent_id: Some(request.parent_agent_id.clone()),
                subagent_role_id: request.subagent_role_id.clone(),
                loop_state: None,
                memory_snapshot: None,
                tool_manifest: None,
                last_error: None,
                created_at_ms: now_ms,
                updated_at_ms: now_ms,
            },
        );
        session.updated_at_ms = now_ms;
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot).await;
        self.apply_host_actions_internal(request.session_id.as_str(), decision.actions)
            .await?;
        Ok(decision.result)
    }

    pub async fn join_subagent(
        self: &Arc<Self>,
        request: JoinSubagentRequest,
    ) -> Result<JoinSubagentResult, SubagentControlError> {
        self.ensure_session_match(&request.session_id).await?;

        let now_ms = current_time_ms();
        let mut session = self.session.lock().await;
        let decision = self
            .coordinator
            .join(&mut session.subagent_state, &request, now_ms)?;
        session.updated_at_ms = now_ms;
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot).await;

        match decision {
            subagent::JoinDecision::Immediate { result, actions } => {
                self.apply_host_actions_internal(request.session_id.as_str(), actions)
                    .await?;
                Ok(result)
            }
            subagent::JoinDecision::Pending { result, actions } => {
                self.apply_host_actions_internal(request.session_id.as_str(), actions)
                    .await?;
                Ok(result)
            }
        }
    }

    pub(crate) async fn run_root_turn(
        self: &Arc<Self>,
        request: AppTurnRequest,
        resolved_runtime: ResolvedSessionRuntime,
        loop_event_sink_override: Option<Arc<dyn LoopEventSink>>,
        interaction_handle_override: Option<Arc<dyn InteractionHandle>>,
        channel_file_sender_override: Option<Arc<dyn ChannelFileSender>>,
        cancellation_token: Option<CancellationToken>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let _guard = self.root_turn_lock.lock().await;
        self.set_session_status(SessionLifecycleStatus::Running, None)
            .await;

        // Store the parent turn's sinks so spawned subagent lanes can
        // inherit them in remote (daemon) mode. Store the MERGED loop
        // sink so subagents inherit the same effective sink the root
        // lane uses.
        let stored_root_sinks = RootTurnSinks {
            loop_event_sink: super::session_worker::merge_loop_event_sinks(
                resolved_runtime.bindings.loop_event_sink.clone(),
                loop_event_sink_override.clone(),
            ),
            tool_event_sink: resolved_runtime.bindings.tool_event_sink.clone(),
            // Store the parent turn's real interaction handle so spawned
            // subagent lanes can route their `ask_user_question` through it
            // via `request_interaction`. The root lane itself still uses the
            // override directly (injected into its worker), so this does not
            // affect root-lane interaction.
            interaction_handle: interaction_handle_override.clone(),
        };
        *self.current_root_sinks.lock().await = Some(stored_root_sinks);

        let root_agent_id = {
            let session = self.session.lock().await;
            session.runtime.agent_id.clone()
        };
        let mut runtime_input = SessionRuntimeBuildInput::from_turn_request(&request);
        // The service already validated/bound API bootstrap paths before
        // entering the supervisor. Any re-resolve after a suspended tool call
        // must inherit the snapshot and never touch the host source again.
        runtime_input.workspace = None;
        runtime_input.skills = None;
        let result = self
            .run_lane_until_terminal(LaneRunInput {
                agent_id: root_agent_id.clone(),
                runtime_input,
                resolved_runtime: Some(resolved_runtime),
                user_message: request.text,
                append_user_message: true,
                reasoning_effort: request.reasoning_effort,
                loop_event_sink_override,
                interaction_handle_override,
                channel_file_sender_override,
                cancellation_token,
                command_context: request.command_context,
            })
            .await;

        // Clear stored sinks so a subsequent root turn does not inherit
        // stale ones. Cleared in all branches since this is the single
        // exit point for `run_root_turn`.
        *self.current_root_sinks.lock().await = None;

        match result {
            Ok(terminal) => {
                self.set_session_status(SessionLifecycleStatus::Idle, None)
                    .await;
                Ok(terminal.result)
            }
            Err(SessionServiceError::CoreRunWithState {
                message,
                partial_loop_state,
                partial_memory_snapshot,
                tool_manifest,
            }) => {
                tracing::info!(
                    session_id = %request.session_id,
                    agent_id = %root_agent_id,
                    messages_count = partial_loop_state.messages.len(),
                    "persisting partial state after core error"
                );

                self.persist_lane_state(
                    &root_agent_id,
                    Some(partial_loop_state),
                    Some(partial_memory_snapshot),
                    Some(tool_manifest),
                    None,
                )
                .await?;

                self.set_session_status(SessionLifecycleStatus::Failed, Some(message.clone()))
                    .await;
                Err(SessionServiceError::CoreRun { message })
            }
            Err(error) => {
                self.set_session_status(SessionLifecycleStatus::Failed, Some(error.to_string()))
                    .await;
                Err(error)
            }
        }
    }

    async fn run_lane_until_terminal(
        self: &Arc<Self>,
        input: LaneRunInput,
    ) -> Result<LaneTerminal, SessionServiceError> {
        let mut user_message = input.user_message;
        let mut append_user_message = input.append_user_message;
        let mut loop_state = self.load_lane_loop_state(&input.agent_id).await?;
        let mut memory_snapshot = self.load_lane_memory_snapshot(&input.agent_id).await?;
        let mut tool_manifest = self.load_lane_tool_manifest(&input.agent_id).await?;
        let mut next_resolved_runtime = input.resolved_runtime;

        // Determine once whether this lane is the root lane; the root
        // agent_id is stable for the session's lifetime.
        let is_root_lane = {
            let session = self.session.lock().await;
            input.agent_id == session.runtime.agent_id
        };

        loop {
            let mut resolved_runtime = match next_resolved_runtime.take() {
                Some(resolved_runtime) => resolved_runtime,
                None => {
                    let session_snapshot = self.snapshot().await;
                    self.runtime_resolver
                        .resolve(&input.runtime_input, Some(&session_snapshot))
                        .await?
                }
            };

            // Inject the parent turn's sinks for non-root lanes in
            // remote mode, where the resolver returns default (None)
            // bindings.
            if !is_root_lane {
                if let Some(ref sinks) = *self.current_root_sinks.lock().await {
                    if resolved_runtime.bindings.loop_event_sink.is_none() {
                        resolved_runtime.bindings.loop_event_sink = sinks.loop_event_sink.clone();
                    }
                    if resolved_runtime.bindings.tool_event_sink.is_none() {
                        resolved_runtime.bindings.tool_event_sink = sinks.tool_event_sink.clone();
                    }
                }
            }

            // Merge the override into the bindings-level sink so
            // "Waiting for subagents..." messages reach SSE in remote mode.
            let loop_event_sink = super::session_worker::merge_loop_event_sinks(
                resolved_runtime.bindings.loop_event_sink.clone(),
                input.loop_event_sink_override.clone(),
            );
            let operation_backend = self.lease_backend_for_lane(&resolved_runtime).await?;
            crate::gateway::finalize_e2b_runtime(
                &mut resolved_runtime,
                Arc::clone(&operation_backend),
            )
            .await?;
            let session_snapshot = self.snapshot().await;
            let worker_result = SessionWorker::run(SessionWorkerInput {
                runtime_input: input.runtime_input.clone(),
                resolved_runtime,
                session: session_snapshot,
                agent_id: input.agent_id.clone(),
                operation_backend,
                user_message,
                append_user_message,
                reasoning_effort: input.reasoning_effort,
                loop_event_sink_override: input.loop_event_sink_override.clone(),
                interaction_handle_override: input.interaction_handle_override.clone(),
                channel_file_sender_override: input.channel_file_sender_override.clone(),
                loop_state: loop_state.clone(),
                memory_snapshot: memory_snapshot.clone(),
                tool_manifest: tool_manifest.clone(),
                cancellation_token: input.cancellation_token.clone(),
                command_context: input.command_context.clone(),
            })
            .await?;

            loop_state = Some(worker_result.loop_state.clone());
            memory_snapshot = Some(worker_result.memory_snapshot.clone());
            tool_manifest = Some(worker_result.tool_manifest.clone());
            self.persist_lane_state(
                &input.agent_id,
                loop_state.clone(),
                memory_snapshot.clone(),
                tool_manifest.clone(),
                None,
            )
            .await?;

            match worker_result.loop_result {
                RuntimeOutput::Complete(outcome) => {
                    let terminal = terminal_from_outcome(
                        outcome,
                        worker_result.loop_state,
                        worker_result.memory_snapshot,
                    );
                    self.persist_lane_state(
                        &input.agent_id,
                        Some(terminal.loop_state.clone()),
                        Some(terminal.memory_snapshot.clone()),
                        tool_manifest.clone(),
                        None,
                    )
                    .await?;
                    return Ok(terminal);
                }
                RuntimeOutput::Suspended(suspended_calls) => {
                    let mut resumed_loop_state =
                        loop_state
                            .clone()
                            .ok_or_else(|| SessionServiceError::CoreRun {
                                message: format!(
                                    "suspended lane '{}' is missing persisted loop state",
                                    input.agent_id
                                ),
                            })?;

                    let join_ids: Vec<String> = suspended_calls
                        .iter()
                        .map(|call| suspended_join_id(call))
                        .collect::<Result<Vec<_>, SessionServiceError>>()?;

                    let receivers: Vec<oneshot::Receiver<SubagentTerminalSnapshot>> =
                        futures::future::try_join_all(
                            join_ids
                                .iter()
                                .map(|join_id| self.take_join_receiver(join_id)),
                        )
                        .await?;

                    if let Some(sink) = loop_event_sink.as_ref() {
                        sink.on_assistant_message(
                            &input.agent_id,
                            &format!("Waiting for {} subagents to complete...", join_ids.len()),
                        );
                    }

                    let total_count = join_ids.len();
                    let mut futures_stream = futures::stream::FuturesUnordered::new();

                    for (idx, receiver) in receivers.into_iter().enumerate() {
                        futures_stream.push(async move { (idx, receiver.await) });
                    }

                    use futures::StreamExt;
                    let mut completed_results: Vec<(
                        usize,
                        Result<SubagentTerminalSnapshot, oneshot::error::RecvError>,
                    )> = Vec::with_capacity(total_count);

                    while let Some((idx, terminal_result)) = futures_stream.next().await {
                        completed_results.push((idx, terminal_result));

                        if let Some(sink) = loop_event_sink.as_ref() {
                            sink.on_assistant_message(
                                &input.agent_id,
                                &format!(
                                    "Progress: {} of {} subagents completed",
                                    completed_results.len(),
                                    total_count
                                ),
                            );
                        }
                    }

                    completed_results.sort_by_key(|(idx, _)| *idx);

                    for (idx, terminal_result) in completed_results {
                        let suspended_call = &suspended_calls[idx];
                        let join_id = &join_ids[idx];

                        self.remove_pending_join(join_id).await;

                        let (tool_result_msg, output_preview, is_error) = match terminal_result {
                            Ok(terminal) => {
                                let msg = build_join_tool_result_message(
                                    suspended_call,
                                    terminal.clone(),
                                )?;
                                let preview = serde_json::to_string(
                                    &serde_json::json!({ "terminal": terminal }),
                                )
                                .unwrap_or_default();
                                let error = terminal.status == SubagentTerminalKind::Failed;
                                (msg, preview, error)
                            }
                            Err(_) => {
                                let error_msg =
                                    format!("pending join receiver dropped before wake: {join_id}");
                                let msg = build_tool_result_message(&ToolExecutionResult::Failed {
                                    final_call: suspended_call.final_call.clone(),
                                    pre_hook_results: Vec::new(),
                                    error_hook_results: Vec::new(),
                                    execution_error: ToolExecutionError::ExecutionFailed {
                                        message: error_msg.clone(),
                                    },
                                });
                                (msg, error_msg, true)
                            }
                        };

                        resumed_loop_state.messages.push(tool_result_msg);

                        if let Some(sink) = loop_event_sink.as_ref() {
                            sink.on_tool_result(
                                &input.agent_id,
                                &agent_types::events::ToolResultEvent {
                                    call_id: suspended_call.final_call.call_id.clone(),
                                    tool_name: suspended_call.final_call.tool_name.clone(),
                                    output_preview,
                                    is_error,
                                    args_preview: serde_json::to_string_pretty(
                                        &suspended_call.final_call.input,
                                    )
                                    .unwrap_or_else(|_| {
                                        suspended_call.final_call.input.to_string()
                                    }),
                                },
                            );
                        }
                    }

                    drop_unanswered_tool_uses(&mut resumed_loop_state.messages);

                    loop_state = Some(resumed_loop_state.clone());
                    self.persist_lane_state(
                        &input.agent_id,
                        Some(resumed_loop_state),
                        memory_snapshot.clone(),
                        tool_manifest.clone(),
                        None,
                    )
                    .await?;
                    user_message = String::new();
                    append_user_message = false;
                }
            }
        }
    }

    async fn lease_backend_for_lane(
        &self,
        resolved: &ResolvedSessionRuntime,
    ) -> Result<Arc<dyn OperationBackend>, SessionServiceError> {
        let mut session_snapshot = self.snapshot().await;

        // If the in-memory snapshot thinks the session has a backend but the
        // backend was evicted (by this process's eviction path or by another
        // process), the snapshot may not reflect the Paused status that
        // batch_mark_paused_due_to_eviction wrote to the session-store.
        // Reload from the store so lease_session_backend takes the resume-
        // from-checkpoint path instead of creating a fresh (stateless)
        // backend.
        if session_snapshot.backend_instance.is_some()
            && self
                .backend_manager
                .lease_bound_session(&session_snapshot.session_id)
                .await
                .is_err()
        {
            if let Some(store_session) = self.session_store.load(&session_snapshot.session_id).await
            {
                session_snapshot = store_session;
            }
        }

        let lease = lease_session_backend(
            self.backend_manager.as_ref(),
            &session_snapshot,
            resolved,
            self.session_store.clone(),
        )
        .await?;

        let operation_backend = lease.backend();
        let mut session = self.session.lock().await;
        if sync_session_backend_instance(&mut session, &lease) {
            session.updated_at_ms = current_time_ms();
            let snapshot = session.clone();
            drop(session);
            self.session_store.save(snapshot).await;
        }

        // Backend is now bound. Re-report Running so the shared registry
        // flips from its default "idle" to "running": `report_session_status`
        // silently no-ops if no lease exists yet (it didn't before this turn
        // leased one), but now that the binding is in place the call will
        // succeed and update the registry entry's session status. This closes
        // the race where another process evicts the freshly-bound sandbox
        // before the turn re-reports Running. Only e2b/conch backends are
        // tracked in the shared registry; local backends skip the registry
        // write (avoiding needless `IN_PROCESS_LOCK` + flock contention on
        // the local-backend hot path).
        let should_report_running = resolved
            .operation_backend
            .as_ref()
            .map(|config| crate::backend::BackendManager::is_counted_kind(&config.kind))
            .unwrap_or(false);
        if should_report_running {
            let session_id_for_report = self.snapshot().await.session_id.clone();
            self.report_session_status(&session_id_for_report, &SessionLifecycleStatus::Running)
                .await;
        }

        Ok(operation_backend)
    }

    async fn apply_host_actions_internal(
        self: &Arc<Self>,
        session_id: &str,
        actions: Vec<HostAction>,
    ) -> Result<(), SubagentControlError> {
        self.ensure_session_match(session_id).await?;

        for action in actions {
            match action {
                HostAction::SpawnWorker {
                    agent_id,
                    parent_agent_id,
                    description: _,
                    prompt,
                    output_schema: _,
                    max_turns,
                } => {
                    self.spawn_subagent_task(agent_id, parent_agent_id, prompt, max_turns);
                }
                HostAction::SuspendWaiter {
                    join_id,
                    waiter_agent_id: _,
                    target_agent_id: _,
                } => {
                    self.register_pending_join(join_id).await?;
                }
                HostAction::WakeWaiter {
                    join_id,
                    waiter_agent_id: _,
                    terminal,
                } => {
                    self.wake_waiter(join_id, terminal).await?;
                }
                HostAction::EnqueueMailboxItem { item } => {
                    let mut session = self.session.lock().await;
                    session.subagent_state.mailbox.push_back(item);
                    session.updated_at_ms = current_time_ms();
                    let snapshot = session.clone();
                    drop(session);
                    self.session_store.save(snapshot).await;
                }
                HostAction::RequestInteraction { request_id, .. } => {
                    tracing::warn!(
                        request_id = %request_id,
                        "HostAction::RequestInteraction is not supported through HostAction path. \
                         SubagentInteractionHandle directly calls request_interaction() method."
                    );
                }
                HostAction::DeliverInteractionResponse { request_id, .. } => {
                    tracing::warn!(
                        request_id = %request_id,
                        "HostAction::DeliverInteractionResponse is not expected through HostAction path. \
                         Response delivery is handled internally by request_interaction spawn."
                    );
                }
            }
        }

        Ok(())
    }

    fn spawn_subagent_task(
        self: &Arc<Self>,
        agent_id: AgentId,
        parent_agent_id: AgentId,
        prompt: String,
        max_turns: Option<u32>,
    ) {
        let supervisor = Arc::clone(self);
        tokio::spawn(async move {
            let runtime_input = {
                let session = supervisor.snapshot().await;
                runtime_input_from_session(&session, agent_id.clone(), max_turns)
            };
            let interaction_handle =
                Arc::new(super::subagent_interaction::SubagentInteractionHandle::new(
                    Arc::clone(&supervisor),
                    agent_id.clone(),
                    parent_agent_id.clone(),
                )) as Arc<dyn InteractionHandle>;
            let result = supervisor
                .run_lane_until_terminal(LaneRunInput {
                    agent_id: agent_id.clone(),
                    runtime_input,
                    resolved_runtime: None,
                    user_message: prompt,
                    append_user_message: true,
                    reasoning_effort: ReasoningEffort::Off,
                    loop_event_sink_override: None,
                    interaction_handle_override: Some(interaction_handle),
                    channel_file_sender_override: None,
                    cancellation_token: None,
                    command_context: None,
                })
                .await;

            match result {
                Ok(terminal) => {
                    if let Err(error) = supervisor
                        .mark_subagent_terminal(&agent_id, terminal.terminal, None)
                        .await
                    {
                        tracing::error!(
                            agent_id = %agent_id,
                            error = %error,
                            "failed to mark subagent terminal"
                        );
                    }
                }
                Err(error) => {
                    let error_message = error.to_string();
                    let terminal = SubagentTerminalSnapshot {
                        status: SubagentTerminalKind::Failed,
                        reply: None,
                        error: Some(error_message.clone()),
                        completed_at_ms: current_time_ms(),
                    };
                    if let Err(mark_error) = supervisor
                        .mark_subagent_terminal(&agent_id, terminal, Some(error_message))
                        .await
                    {
                        tracing::error!(
                            agent_id = %agent_id,
                            error = %mark_error,
                            "failed to mark subagent failure terminal"
                        );
                    }
                }
            }
        });
    }

    async fn mark_subagent_terminal(
        self: &Arc<Self>,
        agent_id: &AgentId,
        terminal: SubagentTerminalSnapshot,
        last_error: Option<String>,
    ) -> Result<(), SubagentControlError> {
        let mut session = self.session.lock().await;
        let Some(agent_record) = session.agents.get_mut(&agent_id.0) else {
            return Err(SubagentControlError::AgentNotFound {
                agent_id: agent_id.to_string(),
            });
        };
        agent_record.last_error = last_error;
        agent_record.updated_at_ms = terminal.completed_at_ms;
        let actions = self.coordinator.on_terminal(
            &mut session.subagent_state,
            agent_id,
            terminal.clone(),
        )?;
        session.updated_at_ms = terminal.completed_at_ms;
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot).await;
        self.apply_host_actions_internal(self.session_id().await.as_str(), actions)
            .await
    }

    async fn load_lane_loop_state(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<LoopStateSnapshot>, SessionServiceError> {
        let session = self.session.lock().await;
        if *agent_id == session.runtime.agent_id {
            return Ok(session.loop_state.clone());
        }

        let lane = session
            .agents
            .get(&agent_id.0)
            .ok_or_else(|| SessionServiceError::CoreRun {
                message: format!("missing lane state for agent '{}'", agent_id),
            })?;
        Ok(lane.loop_state.clone())
    }

    async fn load_lane_memory_snapshot(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<MemorySnapshot>, SessionServiceError> {
        let session = self.session.lock().await;
        if *agent_id == session.runtime.agent_id {
            return Ok(session.memory_snapshot.clone());
        }

        let lane = session
            .agents
            .get(&agent_id.0)
            .ok_or_else(|| SessionServiceError::CoreRun {
                message: format!("missing memory state for agent '{}'", agent_id),
            })?;
        Ok(lane.memory_snapshot.clone())
    }

    async fn load_lane_tool_manifest(
        &self,
        agent_id: &AgentId,
    ) -> Result<Option<Vec<ToolSpecSnapshot>>, SessionServiceError> {
        let session = self.session.lock().await;
        if *agent_id == session.runtime.agent_id {
            return Ok(session.runtime.tool_manifest.clone());
        }

        let lane = session
            .agents
            .get(&agent_id.0)
            .ok_or_else(|| SessionServiceError::CoreRun {
                message: format!("missing tool manifest state for agent '{}'", agent_id),
            })?;
        Ok(lane.tool_manifest.clone())
    }

    async fn persist_lane_state(
        &self,
        agent_id: &AgentId,
        loop_state: Option<LoopStateSnapshot>,
        memory_snapshot: Option<MemorySnapshot>,
        tool_manifest: Option<Vec<ToolSpecSnapshot>>,
        last_error: Option<String>,
    ) -> Result<(), SessionServiceError> {
        let mut session = self.session.lock().await;
        let now_ms = current_time_ms();
        if *agent_id == session.runtime.agent_id {
            session.loop_state = loop_state;
            session.memory_snapshot = memory_snapshot;
            session.runtime.tool_manifest = tool_manifest;
            session.last_error = last_error;
        } else {
            let lane = session.agents.get_mut(&agent_id.0).ok_or_else(|| {
                SessionServiceError::CoreRun {
                    message: format!("missing lane state for agent '{}'", agent_id),
                }
            })?;
            lane.loop_state = loop_state;
            lane.memory_snapshot = memory_snapshot;
            lane.tool_manifest = tool_manifest;
            lane.last_error = last_error;
            lane.updated_at_ms = now_ms;
        }
        session.updated_at_ms = now_ms;
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot).await;
        Ok(())
    }

    async fn set_session_status(&self, status: SessionLifecycleStatus, last_error: Option<String>) {
        let session_id = {
            let mut session = self.session.lock().await;
            session.status = status.clone();
            session.last_error = last_error;
            session.updated_at_ms = current_time_ms();
            let session_id = session.session_id.clone();
            let snapshot = session.clone();
            drop(session);
            self.session_store.save(snapshot).await;
            session_id
        };

        // Report every status transition so the shared registry stays
        // accurate. Failed/Closed map to "idle" (queue_depth=0) since they
        // are no longer processing; without this the registry would keep
        // a stale "running" status forever, blocking eviction.
        self.report_session_status(&session_id, &status).await;
    }

    async fn report_session_status(&self, session_id: &str, status: &SessionLifecycleStatus) {
        let lease = match self.backend_manager.lease_bound_session(session_id).await {
            Ok(lease) => lease,
            Err(_) => return,
        };

        let backend_id = &lease.instance().backend_id.0;

        let (status_str, queue_depth) = match status {
            SessionLifecycleStatus::Idle => ("idle", 0),
            SessionLifecycleStatus::Running => ("running", 1),
            // Failed, Closed, and Paused sessions are done processing.
            // Report them as idle so the backend can be evicted promptly.
            _ => ("idle", 0),
        };

        self.backend_manager
            .registry()
            .update_session_status(backend_id, session_id, status_str, queue_depth)
            .await
            .ok();
    }

    async fn ensure_session_match(&self, session_id: &str) -> Result<(), SubagentControlError> {
        let expected = self.session_id().await;
        if expected == session_id {
            return Ok(());
        }

        Err(SubagentControlError::SessionMismatch {
            expected,
            actual: session_id.to_string(),
        })
    }

    async fn session_id(&self) -> String {
        self.session.lock().await.session_id.clone()
    }

    async fn register_pending_join(&self, join_id: String) -> Result<(), SubagentControlError> {
        let mut pending_joins = self.pending_joins.lock().await;
        if pending_joins.contains_key(&join_id) {
            return Err(SubagentControlError::InvalidState {
                message: format!("duplicate pending join registration: {join_id}"),
            });
        }

        let (sender, receiver) = oneshot::channel();
        pending_joins.insert(
            join_id,
            PendingJoinWaiter {
                sender: Some(sender),
                receiver: Some(receiver),
            },
        );
        Ok(())
    }

    async fn take_join_receiver(
        &self,
        join_id: &str,
    ) -> Result<oneshot::Receiver<SubagentTerminalSnapshot>, SessionServiceError> {
        let mut pending_joins = self.pending_joins.lock().await;
        let pending_join =
            pending_joins
                .get_mut(join_id)
                .ok_or_else(|| SessionServiceError::CoreRun {
                    message: format!("missing pending join registration: {join_id}"),
                })?;
        pending_join
            .receiver
            .take()
            .ok_or_else(|| SessionServiceError::CoreRun {
                message: format!("pending join receiver already taken: {join_id}"),
            })
    }

    async fn wake_waiter(
        &self,
        join_id: String,
        terminal: SubagentTerminalSnapshot,
    ) -> Result<(), SubagentControlError> {
        let mut pending_joins = self.pending_joins.lock().await;
        let pending_join =
            pending_joins
                .get_mut(&join_id)
                .ok_or_else(|| SubagentControlError::InvalidState {
                    message: format!("pending join missing during wake: {join_id}"),
                })?;
        let sender =
            pending_join
                .sender
                .take()
                .ok_or_else(|| SubagentControlError::InvalidState {
                    message: format!("pending join sender already consumed: {join_id}"),
                })?;
        sender
            .send(terminal)
            .map_err(|_| SubagentControlError::Unavailable {
                message: format!("failed to deliver wake signal for join: {join_id}"),
            })
    }

    async fn remove_pending_join(&self, join_id: &str) {
        self.pending_joins.lock().await.remove(join_id);
    }
}

fn runtime_input_from_session(
    session: &SessionRecord,
    agent_id: AgentId,
    max_turns_override: Option<u32>,
) -> SessionRuntimeBuildInput {
    let is_subagent = agent_id != session.runtime.agent_id;
    let subagent_role_id = session
        .agents
        .get(&agent_id.0)
        .and_then(|record| record.subagent_role_id.clone());
    SessionRuntimeBuildInput {
        session_id: session.session_id.clone(),
        conversation_id: session.conversation_id.clone(),
        sender_id: session.sender_id.clone(),
        channel: session.channel.clone(),
        channel_instance_id: session.channel_instance_id.clone(),
        channel_identity_prompt: None,
        entry: session.entry.clone(),
        agent_id_override: if is_subagent { Some(agent_id) } else { None },
        max_turns_override,
        subagent_role_id,
        llm: session.runtime.llm.clone(),
        workspace: None,
        skills: None,
    }
}

fn terminal_from_outcome(
    outcome: AgentOutcome,
    loop_state: LoopStateSnapshot,
    memory_snapshot: MemorySnapshot,
) -> LaneTerminal {
    let completed_at_ms = current_time_ms();

    let (status, reply, messages, token_usage, estimated_input_tokens, outcome) = match outcome {
        AgentOutcome::Complete {
            reply,
            messages,
            token_usage,
            estimated_input_tokens,
            ..
        } => (
            SubagentTerminalKind::Completed,
            reply,
            messages,
            token_usage,
            estimated_input_tokens,
            TurnOutcome::Complete,
        ),
        AgentOutcome::MaxTurnsReached {
            partial_reply,
            messages,
            token_usage,
            estimated_input_tokens,
            ..
        } => (
            SubagentTerminalKind::MaxTurnsReached,
            partial_reply.unwrap_or_default(),
            messages,
            token_usage,
            estimated_input_tokens,
            TurnOutcome::MaxTurnsReached,
        ),
        AgentOutcome::BudgetExhausted {
            partial_reply,
            messages,
            token_usage,
            estimated_input_tokens,
            ..
        } => (
            SubagentTerminalKind::BudgetExhausted,
            partial_reply.unwrap_or_default(),
            messages,
            token_usage,
            estimated_input_tokens,
            TurnOutcome::BudgetExhausted,
        ),
        AgentOutcome::Cancelled {
            partial_reply,
            messages,
            token_usage,
            estimated_input_tokens,
            ..
        } => (
            SubagentTerminalKind::Cancelled,
            partial_reply.unwrap_or_default(),
            messages,
            token_usage,
            estimated_input_tokens,
            TurnOutcome::Cancelled,
        ),
    };

    LaneTerminal {
        result: AppTurnResult {
            raw_reply: reply.clone(),
            visible_reply: reply.clone(),
            messages,
            prompt_tokens: token_usage.prompt_tokens as u64,
            completion_tokens: token_usage.completion_tokens as u64,
            total_tokens: token_usage.total_tokens as u64,
            estimated_input_tokens: estimated_input_tokens as u64,
            outcome,
            hook_actions: Vec::new(),
        },
        terminal: SubagentTerminalSnapshot {
            status,
            reply: Some(reply),
            error: None,
            completed_at_ms,
        },
        loop_state,
        memory_snapshot,
    }
}

/// Remove assistant `ToolUse` blocks whose `call_id` has no matching `ToolResult`
/// anywhere in the history, so a resumed conversation never sends a dangling
/// tool_use (which providers reject). After every suspended call of a turn is
/// resolved this is a no-op; it only fires for a sibling stranded by a stop
/// short-circuit in the same batch.
fn drop_unanswered_tool_uses(messages: &mut [agent_types::ChatMessage]) {
    use agent_types::llm::{ContentBlock, MessageRole};
    let answered: std::collections::HashSet<String> = messages
        .iter()
        .flat_map(|m| m.blocks.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.clone()),
            _ => None,
        })
        .collect();
    for message in messages.iter_mut() {
        if matches!(message.role, MessageRole::Assistant) {
            message.blocks.retain(|b| match b {
                ContentBlock::ToolUse { call_id, .. } => answered.contains(call_id),
                _ => true,
            });
        }
    }
}

fn suspended_join_id(suspended_call: &SuspendedToolCall) -> Result<String, SessionServiceError> {
    match &suspended_call.reason {
        LoopSuspendReason::ToolCall {
            tool_name,
            suspend_token,
        } if tool_name == "join_subagent" => Ok(suspend_token.clone()),
        LoopSuspendReason::ToolCall { tool_name, .. } => Err(SessionServiceError::CoreRun {
            message: format!("unexpected suspended tool while waiting on lane: {tool_name}"),
        }),
    }
}

fn build_join_tool_result_message(
    suspended_call: &SuspendedToolCall,
    terminal: SubagentTerminalSnapshot,
) -> Result<agent_types::ChatMessage, SessionServiceError> {
    let output = serde_json::to_string(&json!({ "terminal": terminal })).map_err(|error| {
        SessionServiceError::CoreRun {
            message: format!("failed to serialize join_subagent output: {error}"),
        }
    })?;
    Ok(build_tool_result_message(&ToolExecutionResult::Completed {
        final_call: suspended_call.final_call.clone(),
        raw_outcome: RawToolOutcome::Success { output },
        pre_hook_results: Vec::new(),
        post_hook_results: Vec::new(),
    }))
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::{
        InMemorySessionStore, SessionRuntimeBuildInput, SessionRuntimeResolveError,
    };
    use std::collections::BTreeMap;

    /// Pins that `SessionSupervisor::new` initializes both sink fields
    /// to `None`, so `run_lane_until_terminal`'s `is_none()` guard
    /// correctly skips injection when no root turn is in progress.
    #[tokio::test]
    async fn new_initializes_sink_inheritance_fields_to_none() {
        let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
        let resolver: Arc<dyn SessionRuntimeResolver> = Arc::new(NoopRuntimeResolver);
        let backend_manager = Arc::new(BackendManager::new());
        let session = SessionRecord {
            session_id: "test-session".to_string(),
            conversation_id: "test-session".to_string(),
            sender_id: "test-user".to_string(),
            entry: crate::gateway::GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
            status: SessionLifecycleStatus::Idle,
            runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
                agent_id: AgentId("root-agent".to_string()),
                model: "stub-model".to_string(),
                llm: None,
                system_prompt: String::new(),
                feature_flags: agent_types::context::FeatureFlags::default(),
                token_budget: agent_types::context::TokenBudgetConfig {
                    total_budget: 4096,
                    reserved_for_output: 1024,
                    reserved_for_system: 256,
                    hard_limit_ratio: 0.9,
                },
                workspace_root: std::path::PathBuf::from("/tmp"),
                max_turns: None,
                tool_manifest: None,
                subagent_roles: BTreeMap::new(),
                bootstrap_binding: None,
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
            created_at_ms: 0,
            updated_at_ms: 0,
        };
        let supervisor = SessionSupervisor::new(store, resolver, backend_manager, session, None);

        assert!(
            supervisor.current_root_sinks.lock().await.is_none(),
            "current_root_sinks should start as None"
        );
    }

    /// Stub resolver for tests that do not exercise `resolve()`.
    struct NoopRuntimeResolver;

    #[async_trait::async_trait]
    impl SessionRuntimeResolver for NoopRuntimeResolver {
        async fn resolve(
            &self,
            _request: &SessionRuntimeBuildInput,
            _existing: Option<&SessionRecord>,
        ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
            Err(SessionRuntimeResolveError::ResolveFailed {
                message: "NoopRuntimeResolver does not resolve".to_string(),
            })
        }
    }
}
