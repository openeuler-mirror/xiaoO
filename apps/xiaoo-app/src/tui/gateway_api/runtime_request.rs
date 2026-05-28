use std::collections::BTreeSet;
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
use tool::{load_tool_sources_with_services, ToolRuntimeServices};

use super::runtime::GatewayRuntime;
use xiaoo_core::spawn_prefetch;

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../../prompts/tui_default_system_prompt.txt");

impl GatewayRuntime {
    pub async fn start_turn(&mut self, state: &mut AppState, prompt: String) -> Result<(), String> {
        self.start_turn_internal(state, prompt, true).await
    }

    pub async fn start_next_queued_turn(&mut self, state: &mut AppState) -> Result<bool, String> {
        if state.chat_state.is_loading {
            return Ok(false);
        }
        let Some(queued) = state.chat_state.pop_pending_turn() else {
            return Ok(false);
        };
        self.discard_pending_user_message(&queued.prompt);
        self.start_turn_internal(state, queued.prompt, true).await?;
        Ok(true)
    }

    pub fn enqueue_pending_user_message_for_running_turn(&mut self, prompt: String) -> bool {
        if self.remote.is_some() || self.stream_rx.is_none() {
            return false;
        }

        if let Ok(mut pending) = self.pending_user_messages.lock() {
            pending.push_back(prompt);
            true
        } else {
            false
        }
    }

    fn discard_pending_user_message(&mut self, prompt: &str) {
        let Ok(mut pending) = self.pending_user_messages.lock() else {
            return;
        };
        let Some(index) = pending.iter().position(|queued| queued == prompt) else {
            return;
        };
        pending.remove(index);
    }

    async fn start_turn_internal(
        &mut self,
        state: &mut AppState,
        prompt: String,
        append_user_message: bool,
    ) -> Result<(), String> {
        if self.remote.is_some() {
            return self
                .start_remote_turn(state, prompt, append_user_message)
                .await;
        }

        if let Some(env_name) = state.agent_config.llm.api_key_env.as_deref() {
            let trimmed = env_name.trim();
            if !trimmed.is_empty() {
                let env_value = std::env::var(trimmed).unwrap_or_default();
                if env_value.trim().is_empty() {
                    return Err(format!(
                        "env var {} is not set. Please configure your API key with /connect or set the environment variable.",
                        trimmed
                    ));
                }
            }
        }

        let runtime_config = self.build_runtime_config(state)?;
        let open_request = self.session_open_request(state)?;
        let turn_request = self.turn_request(state, prompt.clone())?;

        state.chat_state.stick_to_bottom = true;
        self.request_start = Some(Instant::now());
        self.first_token_latency_recorded = false;
        if append_user_message {
            state.chat_state.messages.push(Message::user(prompt));
            state.chat_state.input.reset();
        }
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
        let pending_user_messages = self.pending_user_messages.clone();
        let prefetch_session_id = state.session_id.clone();
        let kvcache_enabled = runtime_config.descriptor.feature_flags.kvcache_enabled;
        tokio::spawn(async move {
            if kvcache_enabled {
                if let Some(snapshot) = session_gateway.session_snapshot(&prefetch_session_id).await
                {
                    let chunk_hashes: Vec<String> = snapshot
                        .loop_state
                        .as_ref()
                        .map(|ls| ls.kv_cache_map.chunk_hashes())
                        .unwrap_or_default();
                    spawn_prefetch(chunk_hashes, "turn_prefetch".to_string());
                }
            }

            if let Err(error) = session_gateway
                .ensure_session_open(runtime_config.clone(), open_request)
                .await
            {
                let _ = updates_tx.send(crate::session_gateway::SessionTurnUpdate::Err(error));
                return;
            }
            session_gateway.spawn_turn(
                runtime_config,
                turn_request,
                updates_tx,
                interaction_rx,
                pending_user_messages,
            );
        });

        Ok(())
    }

    fn build_runtime_config(&self, state: &AppState) -> Result<HostedSessionRuntimeConfig, String> {
        let agent_id = resolve_agent_id(None, None, &state.agent_config)?;
        let system_prompt = state
            .active_agent_role_config()
            .and_then(|role| role.prompt.clone())
            .unwrap_or_else(|| {
                DEFAULT_SYSTEM_PROMPT
                    .trim_end_matches(['\r', '\n'])
                    .to_string()
            });
        let total_budget =
            crate::config::resolve_context_window(&state.agent_config).ok_or_else(|| {
                "invalid TUI runtime state: unable to resolve context window".to_string()
            })?;
        let reserved_for_output = usize::try_from(state.agent_config.llm.max_tokens)
            .map_err(|_| "invalid TUI runtime state: invalid [llm].max_tokens".to_string())?;

        Ok(HostedSessionRuntimeConfig {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId(agent_id),
                model: state.agent_config.llm.model.clone(),
                system_prompt,
                feature_flags: {
                    let mut flags = FeatureFlags::default();
                    flags.kvcache_enabled = state.agent_config.llm.kvcache_enabled;
                    flags.kvcache_debug_enabled = state.agent_config.llm.kvcache_debug_enabled;
                    flags
                },
                token_budget: TokenBudgetConfig {
                    total_budget,
                    reserved_for_output,
                    reserved_for_system: reserved_for_output,
                    hard_limit_ratio: 1.0,
                },
                workspace_root: state.workspace.clone(),
                max_turns: state
                    .active_agent_role_config()
                    .and_then(|role| role.max_turns),
                subagent_roles: state
                    .agent_config
                    .subagent
                    .iter()
                    .map(|(role_id, config)| {
                        (role_id.clone(), crate::gateway::session_record::SubagentRoleRecord {
                            role_id: role_id.clone(),
                            description: config.description.clone(),
                            prompt: config.prompt.clone(),
                            max_turns: config.max_turns,
                            tools: config.tools.clone(),
                        })
                    })
                    .collect(),
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
            visible_tool_names: resolve_visible_tool_names(state),
            compression_pipeline: None,
            llm_provider: None,
            trace: state
                .agent_config
                .trace
                .clone()
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
            hooker: state.agent_config.hooker.clone(),
            operation_backend: state.agent_config.operation_backend.clone(),
            lsp_registry: state.agent_config.build_lsp_registry(),
            skills_config: state.agent_config.resolve_skills_config(),
        })
    }

    fn session_open_request(&self, state: &AppState) -> Result<SessionOpenRequest, String> {
        let sender_id = resolve_agent_id(None, None, &state.agent_config)?;
        Ok(SessionOpenRequest {
            session_id: state.session_id.clone(),
            conversation_id: state.session_id.clone(),
            sender_id,
            entry: tui_entry_context(state),
            channel: None,
            channel_instance_id: None,
        })
    }

    fn turn_request(&self, state: &AppState, text: String) -> Result<AppTurnRequest, String> {
        let sender_id = resolve_agent_id(None, None, &state.agent_config)?;
        Ok(AppTurnRequest {
            session_id: state.session_id.clone(),
            entry: tui_entry_context(state),
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
            reasoning_effort: state.reasoning_effort,
        })
    }
}

fn tui_entry_context(state: &AppState) -> GatewayEntryContext {
    let mut entry = GatewayEntryContext::tui(None);
    entry.runtime_profile_id = state.active_agent_role.clone();
    entry
}

fn resolve_visible_tool_names(state: &AppState) -> Option<Vec<String>> {
    let role = state.active_agent_role_config()?;
    if role.tools.is_empty() {
        return None;
    }

    let all_tool_names: BTreeSet<String> = load_tool_sources_with_services(ToolRuntimeServices {
        workspace_root: Some(state.workspace.clone()),
        ..ToolRuntimeServices::default()
    })
    .iter()
    .flat_map(|source| source.discover())
    .map(|tool| tool.spec.name().0.clone())
    .collect();
    let mut visible_tool_names = all_tool_names.clone();

    for (configured_name, enabled) in &role.tools {
        if !all_tool_names.contains(configured_name) {
            continue;
        }
        if *enabled {
            visible_tool_names.insert(configured_name.clone());
        } else {
            visible_tool_names.remove(configured_name);
        }
    }

    Some(visible_tool_names.into_iter().collect())
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

#[cfg(test)]
mod tests {
    use super::{resolve_visible_tool_names, tui_entry_context};
    use crate::app_state::AppState;
    use crate::config::{AgentRoleConfig, Config};
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;

    #[test]
    fn resolve_visible_tool_names_requires_exact_tool_names() {
        let mut config = Config::default();
        config.agent.insert(
            "code-reviewer".to_string(),
            AgentRoleConfig {
                description: String::new(),
                prompt: None,
                max_turns: None,
                tools: BTreeMap::from([
                    ("write".to_string(), false),
                    ("file_write".to_string(), false),
                ]),
            },
        );

        let mut state =
            AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
                .expect("app state should initialize");
        state.active_agent_role = Some("code-reviewer".to_string());

        let visible = resolve_visible_tool_names(&state).expect("tool visibility should resolve");
        let visible: BTreeSet<_> = visible.into_iter().collect();

        assert!(visible.contains("file_edit"));
        assert!(!visible.contains("file_write"));
    }

    #[test]
    fn tui_entry_context_carries_active_agent_role() {
        let mut config = Config::default();
        config.agent.insert(
            "plan".to_string(),
            AgentRoleConfig {
                description: String::new(),
                prompt: None,
                max_turns: None,
                tools: BTreeMap::new(),
            },
        );
        let mut state =
            AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
                .expect("app state should initialize");
        state.active_agent_role = Some("plan".to_string());

        let entry = tui_entry_context(&state);

        assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
    }
}
