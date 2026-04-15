use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc::unbounded_channel;

use crate::app_state::AppState;
use crate::chat::Message;
use crate::config::Config;
use crate::gateway::{
    AppTurnRequest, GatewayEntryContext, HostedSessionRuntimeConfig, SessionOpenRequest,
    SessionRuntimeDescriptor,
};
use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};

use super::runtime::GatewayRuntime;

const DEFAULT_SYSTEM_PROMPT: &str = "You are a coding agent.";

impl GatewayRuntime {
    pub async fn start_turn(&mut self, state: &mut AppState, prompt: String) -> Result<(), String> {
        let runtime_config = self.build_runtime_config(state)?;
        let open_request = self.session_open_request(state)?;
        let turn_request = self.turn_request(state, prompt.clone())?;

        state.chat_state.stick_to_bottom = true;
        self.request_start = Some(Instant::now());
        self.first_token_latency_recorded = false;
        state.chat_state.messages.push(Message::user(prompt));
        state.chat_state.input.reset();
        state.chat_state.is_loading = true;
        state
            .chat_state
            .messages
            .push(Message::assistant_streaming());
        self.stream_message_index = Some(state.chat_state.messages.len().saturating_sub(1));
        self.stream_reveal_buffer.clear();
        self.pending_stream_done = None;

        let (updates_tx, updates_rx) = unbounded_channel();
        let (interaction_tx, interaction_rx) = unbounded_channel();
        self.interaction_reply_tx = Some(interaction_tx);
        self.stream_rx = Some(updates_rx);
        self.cancel_flag = Some(Arc::new(AtomicBool::new(false)));

        let session_gateway = self.session_gateway.clone();
        tokio::spawn(async move {
            if let Err(error) = session_gateway
                .ensure_session_open(runtime_config.clone(), open_request)
                .await
            {
                let _ = updates_tx.send(crate::session_gateway::SessionTurnUpdate::Err(error));
                return;
            }
            session_gateway.spawn_turn(runtime_config, turn_request, updates_tx, interaction_rx);
        });

        Ok(())
    }

    fn build_runtime_config(&self, state: &AppState) -> Result<HostedSessionRuntimeConfig, String> {
        let agent_id = resolve_agent_id(None, None, &state.agent_config)?;
        let total_budget = state
            .agent_config
            .llm
            .context_window
            .and_then(|value| usize::try_from(value).ok())
            .filter(|value| *value > 0)
            .ok_or_else(|| "invalid TUI runtime state: missing [llm].context_window".to_string())?;
        let reserved_for_output = usize::try_from(state.agent_config.llm.max_tokens)
            .map_err(|_| "invalid TUI runtime state: invalid [llm].max_tokens".to_string())?;

        Ok(HostedSessionRuntimeConfig {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId(agent_id),
                model: state.agent_config.llm.model.clone(),
                system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
                feature_flags: FeatureFlags::default(),
                token_budget: TokenBudgetConfig {
                    total_budget,
                    reserved_for_output,
                    reserved_for_system: reserved_for_output,
                    hard_limit_ratio: 1.0,
                },
                workspace_root: state.workspace.clone(),
                max_turns: None,
            },
            provider: state.agent_config.llm.provider.clone(),
            model: state.agent_config.llm.model.clone(),
            api_key: None,
            api_key_env: state.agent_config.llm.api_key_env.clone(),
            api_base: if state.agent_config.llm.api_base.trim().is_empty() {
                None
            } else {
                Some(state.agent_config.llm.api_base.clone())
            },
            visible_tool_names: None,
            compression_pipeline: None,
            llm_provider: None,
            trace: state
                .agent_config
                .trace
                .clone()
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
            hooker: state.agent_config.hooker.clone(),
        })
    }

    fn session_open_request(&self, state: &AppState) -> Result<SessionOpenRequest, String> {
        let sender_id = resolve_agent_id(None, None, &state.agent_config)?;
        Ok(SessionOpenRequest {
            session_id: state.session_id.clone(),
            conversation_id: state.session_id.clone(),
            sender_id,
            entry: GatewayEntryContext::tui(None),
            channel: None,
            channel_instance_id: None,
        })
    }

    fn turn_request(&self, state: &AppState, text: String) -> Result<AppTurnRequest, String> {
        let sender_id = resolve_agent_id(None, None, &state.agent_config)?;
        Ok(AppTurnRequest {
            session_id: state.session_id.clone(),
            entry: GatewayEntryContext::tui(None),
            channel: None,
            message_id: None,
            conversation_id: state.session_id.clone(),
            sender_id,
            text,
            channel_instance_id: None,
            channel_identity_prompt: None,
            reply_to_message_id: None,
            root_message_id: None,
            mentions: Vec::new(),
        })
    }
}

pub(super) fn resolve_agent_id(
    explicit_id: Option<&str>,
    session_agent_id: Option<&str>,
    config: &Config,
) -> Result<String, String> {
    let ids = config.list_agent_ids();
    let has_list = !ids.is_empty();

    if let Some(id) = explicit_id.filter(|value| !value.is_empty()) {
        let normalized = id.to_lowercase();
        if has_list && !ids.contains(&normalized) {
            return Err(format!(
                "agent id {:?} not in agents.list (available: {:?})",
                normalized, ids
            ));
        }
        return Ok(normalized);
    }

    if let Some(id) = session_agent_id.filter(|value| !value.is_empty()) {
        let normalized = id.to_lowercase();
        if has_list && !ids.contains(&normalized) {
            return Err(format!(
                "session agent id {:?} not in agents.list (available: {:?})",
                normalized, ids
            ));
        }
        return Ok(normalized);
    }

    config
        .validate_default_agent_id()
        .map_err(|error| error.to_string())?;
    Ok(config.resolve_default_agent_id())
}
