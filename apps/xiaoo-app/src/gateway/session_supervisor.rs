use crate::gateway::session_record::SessionAgentRecord;
use crate::gateway::{
    AppTurnRequest, AppTurnResult, ResolvedSessionRuntime, SessionLifecycleStatus, SessionRecord,
    SessionRuntimeBuildInput, SessionRuntimeResolver, SessionServiceError, SessionStore,
};
use agent_contracts::LoopEventSink;
use agent_types::common::ids::AgentId;
use agent_types::outcome::AgentOutcome;
use agent_types::tool::{RawToolOutcome, ToolExecutionResult};
use memory::MemorySnapshot;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use subagent::{
    HostAction, JoinSubagentRequest, JoinSubagentResult, SpawnSubagentRequest, SpawnSubagentResult,
    SubagentControlError, SubagentCoordinator, SubagentTerminalKind, SubagentTerminalSnapshot,
};
use tokio::sync::{oneshot, Mutex};
use xiaoo_core::agent_loop::build_tool_result_message;
use xiaoo_core::{LoopRunResult, LoopStateSnapshot, LoopSuspendReason, SuspendedToolCall};

use super::session_worker::{SessionWorker, SessionWorkerInput};

struct PendingJoinWaiter {
    sender: Option<oneshot::Sender<SubagentTerminalSnapshot>>,
    receiver: Option<oneshot::Receiver<SubagentTerminalSnapshot>>,
}

struct LaneRunInput {
    agent_id: AgentId,
    runtime_input: SessionRuntimeBuildInput,
    user_message: String,
    append_user_message: bool,
    loop_event_sink_override: Option<Arc<dyn LoopEventSink>>,
}

struct LaneTerminal {
    result: AppTurnResult,
    terminal: SubagentTerminalSnapshot,
    loop_state: LoopStateSnapshot,
    memory_snapshot: MemorySnapshot,
}

pub struct SessionSupervisor {
    session_store: Arc<dyn SessionStore>,
    runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    coordinator: SubagentCoordinator,
    session: Mutex<SessionRecord>,
    pending_joins: Mutex<HashMap<String, PendingJoinWaiter>>,
    root_turn_lock: Mutex<()>,
}

impl SessionSupervisor {
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        session: SessionRecord,
    ) -> Self {
        Self {
            session_store,
            runtime_resolver,
            coordinator: SubagentCoordinator::new(),
            session: Mutex::new(session),
            pending_joins: Mutex::new(HashMap::new()),
            root_turn_lock: Mutex::new(()),
        }
    }

    pub async fn snapshot(&self) -> SessionRecord {
        self.session.lock().await.clone()
    }

    pub async fn prepare_root_turn(
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

    pub async fn force_close(&self) -> SessionRecord {
        let mut session = self.session.lock().await;
        session.status = SessionLifecycleStatus::Closed;
        session.updated_at_ms = current_time_ms();
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot.clone()).await;
        snapshot
    }

    pub async fn spawn_subagent(
        self: &Arc<Self>,
        request: SpawnSubagentRequest,
    ) -> Result<SpawnSubagentResult, SubagentControlError> {
        self.ensure_session_match(&request.session_id).await?;

        let child_agent_id = AgentId(uuid::Uuid::new_v4().to_string());
        let now_ms = current_time_ms();
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
                loop_state: None,
                memory_snapshot: None,
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

    pub async fn run_root_turn(
        self: &Arc<Self>,
        request: AppTurnRequest,
        loop_event_sink_override: Option<Arc<dyn LoopEventSink>>,
    ) -> Result<AppTurnResult, SessionServiceError> {
        let _guard = self.root_turn_lock.lock().await;
        self.set_session_status(SessionLifecycleStatus::Running, None)
            .await;

        let root_agent_id = {
            let session = self.session.lock().await;
            session.runtime.agent_id.clone()
        };
        let runtime_input = SessionRuntimeBuildInput::from_turn_request(&request);
        let result = self
            .run_lane_until_terminal(LaneRunInput {
                agent_id: root_agent_id,
                runtime_input,
                user_message: request.text,
                append_user_message: true,
                loop_event_sink_override,
            })
            .await;

        match result {
            Ok(terminal) => {
                self.set_session_status(SessionLifecycleStatus::Idle, None)
                    .await;
                Ok(terminal.result)
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

        loop {
            let session_snapshot = self.snapshot().await;
            let worker_result = SessionWorker::run(
                self.runtime_resolver.as_ref(),
                SessionWorkerInput {
                    runtime_input: input.runtime_input.clone(),
                    session: session_snapshot,
                    agent_id: input.agent_id.clone(),
                    user_message,
                    append_user_message,
                    loop_event_sink_override: input.loop_event_sink_override.clone(),
                    loop_state: loop_state.clone(),
                    memory_snapshot: memory_snapshot.clone(),
                },
            )
            .await?;

            loop_state = Some(worker_result.loop_state.clone());
            memory_snapshot = Some(worker_result.memory_snapshot.clone());
            self.persist_lane_state(
                &input.agent_id,
                loop_state.clone(),
                memory_snapshot.clone(),
                None,
            )
            .await?;

            match worker_result.loop_result {
                LoopRunResult::Complete(outcome) => {
                    let terminal = terminal_from_outcome(
                        outcome,
                        worker_result.loop_state,
                        worker_result.memory_snapshot,
                    );
                    self.persist_lane_state(
                        &input.agent_id,
                        Some(terminal.loop_state.clone()),
                        Some(terminal.memory_snapshot.clone()),
                        None,
                    )
                    .await?;
                    return Ok(terminal);
                }
                LoopRunResult::Suspended(suspended_call) => {
                    let join_id = suspended_join_id(&suspended_call)?;
                    let receiver = self.take_join_receiver(&join_id).await?;
                    let terminal = receiver.await.map_err(|_| SessionServiceError::CoreRun {
                        message: format!("pending join receiver dropped before wake: {join_id}"),
                    })?;
                    self.remove_pending_join(&join_id).await;

                    let mut resumed_loop_state =
                        loop_state
                            .clone()
                            .ok_or_else(|| SessionServiceError::CoreRun {
                                message: format!(
                                    "suspended lane '{}' is missing persisted loop state",
                                    input.agent_id
                                ),
                            })?;
                    let tool_result_msg =
                        build_join_tool_result_message(&suspended_call, terminal.clone())?;
                    resumed_loop_state.messages.push(tool_result_msg.clone());

                    let mut runtime_input_copy = input.runtime_input.clone();
                    runtime_input_copy.agent_id_override = Some(input.agent_id.clone());
                    if let Ok(resolved) = self
                        .runtime_resolver
                        .resolve(&runtime_input_copy, Some(&*self.session.lock().await))
                        .await
                    {
                        if let Some(sink) = resolved.bindings.loop_event_sink {
                            let output_preview =
                                serde_json::to_string(&serde_json::json!({ "terminal": terminal }))
                                    .unwrap_or_default();
                            let is_error =
                                terminal.status == subagent::SubagentTerminalKind::Failed;
                            sink.on_tool_result(
                                &input.agent_id,
                                &agent_types::events::ToolResultEvent {
                                    call_id: suspended_call.final_call.call_id.clone(),
                                    tool_name: suspended_call.final_call.tool_name.clone(),
                                    output_preview,
                                    is_error,
                                },
                            );
                        }
                    }

                    loop_state = Some(resumed_loop_state.clone());
                    self.persist_lane_state(
                        &input.agent_id,
                        Some(resumed_loop_state),
                        memory_snapshot.clone(),
                        None,
                    )
                    .await?;
                    user_message = String::new();
                    append_user_message = false;
                }
            }
        }
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
                    parent_agent_id: _,
                    description: _,
                    prompt,
                    output_schema: _,
                } => {
                    self.spawn_subagent_task(agent_id, prompt);
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
            }
        }

        Ok(())
    }

    fn spawn_subagent_task(self: &Arc<Self>, agent_id: AgentId, prompt: String) {
        let supervisor = Arc::clone(self);
        tokio::spawn(async move {
            let runtime_input = {
                let session = supervisor.snapshot().await;
                runtime_input_from_session(&session)
            };
            let result = supervisor
                .run_lane_until_terminal(LaneRunInput {
                    agent_id: agent_id.clone(),
                    runtime_input,
                    user_message: prompt,
                    append_user_message: true,
                    loop_event_sink_override: None,
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

    async fn persist_lane_state(
        &self,
        agent_id: &AgentId,
        loop_state: Option<LoopStateSnapshot>,
        memory_snapshot: Option<MemorySnapshot>,
        last_error: Option<String>,
    ) -> Result<(), SessionServiceError> {
        let mut session = self.session.lock().await;
        let now_ms = current_time_ms();
        if *agent_id == session.runtime.agent_id {
            session.loop_state = loop_state;
            session.memory_snapshot = memory_snapshot;
            session.last_error = last_error;
        } else {
            let lane = session.agents.get_mut(&agent_id.0).ok_or_else(|| {
                SessionServiceError::CoreRun {
                    message: format!("missing lane state for agent '{}'", agent_id),
                }
            })?;
            lane.loop_state = loop_state;
            lane.memory_snapshot = memory_snapshot;
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
        let mut session = self.session.lock().await;
        session.status = status;
        session.last_error = last_error;
        session.updated_at_ms = current_time_ms();
        let snapshot = session.clone();
        drop(session);
        self.session_store.save(snapshot).await;
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

fn runtime_input_from_session(session: &SessionRecord) -> SessionRuntimeBuildInput {
    SessionRuntimeBuildInput {
        session_id: session.session_id.clone(),
        conversation_id: session.conversation_id.clone(),
        sender_id: session.sender_id.clone(),
        channel: session.channel.clone(),
        channel_instance_id: session.channel_instance_id.clone(),
        channel_identity_prompt: None,
        entry: session.entry.clone(),
        agent_id_override: None,
    }
}

fn terminal_from_outcome(
    outcome: AgentOutcome,
    loop_state: LoopStateSnapshot,
    memory_snapshot: MemorySnapshot,
) -> LaneTerminal {
    let completed_at_ms = current_time_ms();

    let (status, reply, messages, token_usage) = match outcome {
        AgentOutcome::Complete {
            reply,
            messages,
            token_usage,
            ..
        } => (
            SubagentTerminalKind::Completed,
            reply,
            messages,
            token_usage,
        ),
        AgentOutcome::MaxTurnsReached {
            partial_reply,
            messages,
            token_usage,
            ..
        } => (
            SubagentTerminalKind::MaxTurnsReached,
            partial_reply.unwrap_or_default(),
            messages,
            token_usage,
        ),
        AgentOutcome::BudgetExhausted {
            partial_reply,
            messages,
            token_usage,
            ..
        } => (
            SubagentTerminalKind::BudgetExhausted,
            partial_reply.unwrap_or_default(),
            messages,
            token_usage,
        ),
        AgentOutcome::Cancelled {
            partial_reply,
            messages,
            token_usage,
            ..
        } => (
            SubagentTerminalKind::Cancelled,
            partial_reply.unwrap_or_default(),
            messages,
            token_usage,
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
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
