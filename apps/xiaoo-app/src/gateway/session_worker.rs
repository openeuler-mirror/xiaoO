use crate::builtin_agent_roles::PLAN_AGENT_ID;
use crate::gateway::{
    AppRuntimeFactory, AppRuntimeFactoryError, SessionRecord, SessionRuntimeBuildInput,
    SessionRuntimeResolver, SessionServiceError,
};
use agent_contracts::{ChannelFileSender, InteractionHandle, LoopEventSink};
use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use agent_types::ReasoningEffort;
use memory::{MemoryManager, MemorySnapshot};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tool::ToolSpecSnapshot;
use xiaoo_core::{
    run_agent_loop, AgentLoopInput, LoopRunResult, LoopState, LoopStateSnapshot, LoopStopRule,
};

pub struct SessionWorkerInput {
    pub runtime_input: SessionRuntimeBuildInput,
    pub session: SessionRecord,
    pub agent_id: AgentId,
    pub user_message: String,
    pub append_user_message: bool,
    pub reasoning_effort: ReasoningEffort,
    pub loop_event_sink_override: Option<Arc<dyn LoopEventSink>>,
    pub interaction_handle_override: Option<Arc<dyn InteractionHandle>>,
    pub channel_file_sender_override: Option<Arc<dyn ChannelFileSender>>,
    pub loop_state: Option<LoopStateSnapshot>,
    pub memory_snapshot: Option<MemorySnapshot>,
    pub tool_manifest: Option<Vec<ToolSpecSnapshot>>,
}

pub struct SessionWorkerResult {
    pub loop_result: LoopRunResult,
    pub loop_state: LoopStateSnapshot,
    pub memory_snapshot: MemorySnapshot,
    pub tool_manifest: Vec<ToolSpecSnapshot>,
}

pub struct SessionWorker;

impl SessionWorker {
    pub async fn run(
        runtime_resolver: &dyn SessionRuntimeResolver,
        input: SessionWorkerInput,
    ) -> Result<SessionWorkerResult, SessionServiceError> {
        let is_root_lane = input.agent_id == input.session.runtime.agent_id;
        let mut runtime_input = input.runtime_input.clone();
        runtime_input.agent_id_override = Some(input.agent_id.clone());

        let mut resolved = runtime_resolver
            .resolve(&runtime_input, Some(&input.session))
            .await?;
        if !is_root_lane {
            resolved.bindings.loop_event_sink = None;
            resolved.bindings.tool_event_sink = None;
            resolved.bindings.interaction_handle = None;
            resolved.bindings.pending_user_messages = None;
        } else {
            // Merge overrides: override takes precedence.
            if let Some(override_handle) = input.interaction_handle_override.clone() {
                resolved.bindings.interaction_handle = Some(override_handle);
            }
            if let Some(override_sender) = input.channel_file_sender_override.clone() {
                resolved.bindings.channel_file_sender = Some(override_sender);
            }
            resolved.bindings.loop_event_sink = merge_loop_event_sinks(
                resolved.bindings.loop_event_sink.clone(),
                input.loop_event_sink_override.clone(),
            );
        }

        // Create LoopState first to get shared message storage
        let loop_session_id = input
            .loop_state
            .as_ref()
            .map(|snapshot| snapshot.session_id)
            .unwrap_or_else(uuid::Uuid::new_v4);
        let cancel = CancellationToken::new();
        let mut loop_state = input
            .loop_state
            .clone()
            .map(|snapshot| LoopState::from_snapshot(snapshot, cancel.clone()))
            .unwrap_or_else(|| LoopState::new(loop_session_id));

        // Share message storage with runtime_view
        let messages = loop_state.messages_arc();
        let assembly = AppRuntimeFactory::build(
            &resolved,
            &input.session,
            messages,
            input.tool_manifest.clone(),
        )
        .await?;
        let tool_manifest = assembly.tool_manifest.clone();

        let mut memory_manager = match input.memory_snapshot.clone() {
            Some(snapshot) => MemoryManager::from_snapshot(snapshot),
            None => {
                let memory_session_id = if is_root_lane {
                    input.session.session_id.clone()
                } else {
                    input.agent_id.0.clone()
                };
                MemoryManager::new(memory_session_id, current_time_ms()).map_err(|error| {
                    SessionServiceError::Memory {
                        message: error.to_string(),
                    }
                })?
            }
        };

        let mut loop_input = AgentLoopInput::new(input.user_message)
            .with_agent_id(input.agent_id.clone())
            .with_visible_tools(assembly.visible_tools.clone())
            .with_reasoning_effort(input.reasoning_effort);
        if !input.append_user_message {
            loop_input = loop_input.resume_without_user_message();
        }
        if input.runtime_input.entry.runtime_profile_id.as_deref() == Some(PLAN_AGENT_ID) {
            loop_input = loop_input.with_stop_rules([LoopStopRule::AfterSuccessfulTool {
                tool_name: "todo_write".to_string(),
            }]);
        }
        if let Some(loop_event_sink) = resolved.bindings.loop_event_sink.clone() {
            loop_input = loop_input.with_event_sink(loop_event_sink);
        }
        if let Some(runtime_view) = assembly.runtime_view.clone() {
            loop_input = loop_input.with_runtime_view(runtime_view);
        }
        if let Some(pending_user_messages) = resolved.bindings.pending_user_messages.clone() {
            loop_input = loop_input.with_pending_user_messages(pending_user_messages);
        }

        let loop_result = run_agent_loop(&assembly.runtime, &mut loop_state, loop_input).await;
        let shutdown_result = assembly.shutdown().await;

        let loop_result = match loop_result {
            Ok(loop_result) => loop_result,
            Err(error) => {
                if let Err(shutdown_error) = shutdown_result {
                    tracing::warn!(
                        session_id = %input.session.session_id,
                        agent_id = %input.agent_id,
                        shutdown_error = %shutdown_error,
                        "runtime shutdown failed after loop error"
                    );
                }
                return Err(SessionServiceError::CoreRun {
                    message: error.to_string(),
                });
            }
        };

        if let Err(error) = shutdown_result {
            return Err(SessionServiceError::RuntimeShutdown {
                message: error.to_string(),
            });
        }

        memory_manager.sync_from_loop_state(&loop_state.messages.read(), current_time_ms());

        Ok(SessionWorkerResult {
            loop_result,
            loop_state: loop_state.to_snapshot(),
            memory_snapshot: memory_manager.snapshot().clone(),
            tool_manifest,
        })
    }
}

#[derive(Clone)]
struct FanoutLoopEventSink {
    sinks: Vec<Arc<dyn LoopEventSink>>,
}

impl LoopEventSink for FanoutLoopEventSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        for sink in &self.sinks {
            sink.on_turn_start(agent_id, turn);
        }
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        for sink in &self.sinks {
            sink.on_assistant_message(agent_id, text);
        }
    }

    fn on_assistant_reasoning(&self, agent_id: &AgentId, text: &str) {
        for sink in &self.sinks {
            sink.on_assistant_reasoning(agent_id, text);
        }
    }

    fn on_tool_result(&self, agent_id: &AgentId, event: &ToolResultEvent) {
        for sink in &self.sinks {
            sink.on_tool_result(agent_id, event);
        }
    }

    fn on_loop_end(&self, agent_id: &AgentId, summary: &LoopEndSummary) {
        for sink in &self.sinks {
            sink.on_loop_end(agent_id, summary);
        }
    }
}

fn merge_loop_event_sinks(
    primary: Option<Arc<dyn LoopEventSink>>,
    secondary: Option<Arc<dyn LoopEventSink>>,
) -> Option<Arc<dyn LoopEventSink>> {
    match (primary, secondary) {
        (None, None) => None,
        (Some(sink), None) | (None, Some(sink)) => Some(sink),
        (Some(primary), Some(secondary)) => Some(Arc::new(FanoutLoopEventSink {
            sinks: vec![primary, secondary],
        })),
    }
}

impl From<AppRuntimeFactoryError> for SessionServiceError {
    fn from(value: AppRuntimeFactoryError) -> Self {
        Self::RuntimeBuild {
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
