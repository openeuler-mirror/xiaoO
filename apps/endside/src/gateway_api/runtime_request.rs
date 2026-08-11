use std::collections::BTreeSet;
use std::time::Instant;

use tokio::sync::mpsc::unbounded_channel;

use crate::app_state::AppState;
use crate::chat::Message;
use crate::config::Config;
use crate::gateway::{
    AppTurnRequest, GatewayEntryContext, HostedSessionRuntimeConfig, LlmRuntimeConfig,
    SessionOpenRequest, SessionRuntimeDescriptor,
};
use agent_types::common::ids::AgentId;
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use tool::{load_tool_sources_with_services, ToolRuntimeServices};

use super::runtime::GatewayRuntime;
use xiaoo_core::spawn_prefetch;

/// User-facing system message (with glyph) shown when the daemon reports the
/// session has been taken over by another TUI process.
pub(crate) fn session_taken_over_notice_glyph() -> String {
    "⚠️ This session has been taken over by another xiaoo process. Submissions are disabled until you switch (use /sessions) or create a new session (use /remote)."
        .to_string()
}

/// Plain-text form of [`session_taken_over_notice_glyph`], used as the local
/// short-circuit error from [`GatewayRuntime::submit_turn`] when the heartbeat
/// has already reported the takeover.
pub(crate) fn session_taken_over_notice_plain() -> String {
    "This session has been taken over by another xiaoo process; use /sessions or /remote to switch to a different session."
        .to_string()
}

const DEFAULT_SYSTEM_PROMPT: &str = include_str!("../prompts/tui_default_system_prompt.txt");
const DEFAULT_SYSTEM_TOKEN_RESERVE: usize = 2048;

impl GatewayRuntime {
    pub async fn start_turn(&mut self, state: &mut AppState, prompt: String) -> Result<(), String> {
        self.start_turn_internal(state, prompt, true, None, 0).await
    }

    /// Like `start_turn` but carries slash-command metadata so the
    /// `*.Chat.command.before` hooker can fire with `{ command, arguments }`.
    pub async fn start_turn_for_command(
        &mut self,
        state: &mut AppState,
        prompt: String,
        command_context: agent_types::chat::CommandContext,
    ) -> Result<(), String> {
        self.start_turn_internal(state, prompt, true, Some(command_context), 0)
            .await
    }

    /// Like `start_turn` but carries a `send_prompt` hook action's
    /// `chain_depth`. User-typed turns use [`start_turn`] (depth 0, resets
    /// the chain); this entry point is for turns spawned by a plugin's
    /// `SendPrompt` action so the daemon can track and cap the cross-turn
    /// chain depth.
    pub async fn start_turn_for_hook_prompt(
        &mut self,
        state: &mut AppState,
        prompt: String,
        chain_depth: usize,
    ) -> Result<(), String> {
        self.start_turn_internal(state, prompt, true, None, chain_depth)
            .await
    }

    pub async fn start_next_queued_turn(&mut self, state: &mut AppState) -> Result<bool, String> {
        if state.chat_state.is_loading {
            return Ok(false);
        }
        let Some(queued) = state.chat_state.pop_pending_turn() else {
            return Ok(false);
        };
        self.discard_pending_user_message(&queued.prompt);
        self.start_turn_internal(
            state,
            queued.prompt,
            true,
            queued.command_context,
            queued.chain_depth,
        )
        .await?;
        Ok(true)
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
        command_context: Option<agent_types::chat::CommandContext>,
        chain_depth: usize,
    ) -> Result<(), String> {
        // Refuse the submission locally when the heartbeat has already
        // reported a takeover — saves a wasted RPC and gives a clearer error
        // than the daemon's 409. Gated on remote mode so a stale flag from a
        // prior session can't block local submissions.
        if self.remote.is_some() && state.session_taken_over {
            return Err(session_taken_over_notice_plain());
        }
        if self.remote.is_some() {
            return self
                .start_remote_turn(
                    state,
                    prompt,
                    append_user_message,
                    command_context,
                    chain_depth,
                )
                .await;
        }

        if let Some(env_name) = state.agent_config.llm.api_key_env.as_deref() {
            let trimmed = env_name.trim();
            if !trimmed.is_empty() {
                let has_api_key = if let Some(key) = crate::gateway::get_decrypted_api_key(trimmed)
                {
                    !key.trim().is_empty()
                } else {
                    std::env::var(trimmed)
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false)
                };
                if !has_api_key {
                    return Err(format!(
                        "API key for {} is not set. Please configure your API key with /connect or set the environment variable.",
                        trimmed
                    ));
                }
            }
        }

        // Defensive sync of the TUI's `session_messages` cache from the
        // backend session store. The `Done` update that normally refreshes
        // `session_messages` via `finish_stream_done` can be missed for many
        // reasons while the session is still alive (channel drop, panic
        // recovery, subagent writes to the backend store, Esc cancellation,
        // etc.). Reloading `loop_state.messages` here at turn start keeps the
        // cache fresh so snapshots and subsequent turns see the full
        // conversation history. This is an in-memory load and short-circuits
        // when the cache is already equal to the persisted state.
        if let Some(record) = self.session_snapshot(&state.session_id).await {
            if let Some(loop_state) = record.loop_state.as_ref() {
                if !loop_state.messages.is_empty() && state.session_messages != loop_state.messages
                {
                    state.session_messages = loop_state.messages.clone();
                }
            }
        }

        let runtime_config = self.build_runtime_config(state)?;
        let open_request = self.session_open_request(state)?;
        let turn_request =
            self.turn_request(state, prompt.clone(), command_context, chain_depth)?;

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
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

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
                Some(cancel_token),
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
        let reserved_for_system = DEFAULT_SYSTEM_TOKEN_RESERVE.min(total_budget.saturating_sub(1));

        Ok(HostedSessionRuntimeConfig {
            descriptor: SessionRuntimeDescriptor {
                agent_id: AgentId(agent_id),
                model: state.agent_config.llm.model.clone(),
                llm: Some(llm_runtime_config_from_state(state)),
                system_prompt,
                feature_flags: {
                    let mut flags = FeatureFlags::default();
                    flags.kvcache_enabled = state.agent_config.llm.kvcache_enabled;
                    flags.kvcache_debug_enabled = state.agent_config.llm.kvcache_debug_enabled;
                    flags.redact_secrets_display =
                        state.agent_config.tui.redact_secrets_display;
                    flags
                },
                token_budget: TokenBudgetConfig {
                    total_budget,
                    reserved_for_output,
                    reserved_for_system,
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
                        (
                            role_id.clone(),
                            crate::gateway::session_record::SubagentRoleRecord {
                                role_id: role_id.clone(),
                                description: config.description.clone(),
                                prompt: config.prompt.clone(),
                                max_turns: config.max_turns,
                                tools: config.tools.clone(),
                            },
                        )
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
            subagent_roles: state
                .agent_config
                .subagent
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        crate::gateway::SubagentRoleConfigEntry {
                            description: v.description.clone(),
                            prompt: v.prompt.clone(),
                            max_turns: v.max_turns,
                            tools: v.tools.clone(),
                        },
                    )
                })
                .collect(),
            mcp_servers: state.agent_config.mcp_servers().to_vec(),
            memory_automation: state.agent_config.memory_automation.clone(),
        })
    }

    fn session_open_request(&self, state: &AppState) -> Result<SessionOpenRequest, String> {
        let sender_id = resolve_agent_id(None, None, &state.agent_config)?;
        Ok(SessionOpenRequest {
            session_id: state.session_id.clone(),
            conversation_id: state.session_id.clone(),
            sender_id,
            entry: tui_entry_context(state, None),
            channel: None,
            channel_instance_id: None,
            llm: None,
            workspace: None,
            skills: None,
            client_id: Some(state.client_id.clone()),
            client_pid: Some(std::process::id()),
            client_hostname: self.client_hostname.clone(),
        })
    }

    fn turn_request(
        &self,
        state: &AppState,
        text: String,
        command_context: Option<agent_types::chat::CommandContext>,
        chain_depth: usize,
    ) -> Result<AppTurnRequest, String> {
        let sender_id = resolve_agent_id(None, None, &state.agent_config)?;
        Ok(AppTurnRequest {
            session_id: state.session_id.clone(),
            entry: tui_entry_context(state, None),
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
            llm: None,
            workspace: None,
            skills: None,
            command_context,
            chain_depth,
            client_id: Some(state.client_id.clone()),
        })
    }
}

/// Best-effort hostname for the daemon's lease-holder display. `None` if
/// unresolved — purely informational.
pub(crate) fn hostname() -> Option<String> {
    #[cfg(unix)]
    {
        std::fs::read_to_string("/etc/hostname")
            .ok()
            .and_then(trim_non_empty)
            .or_else(|| std::env::var("HOSTNAME").ok().and_then(trim_non_empty))
    }
    #[cfg(not(unix))]
    {
        std::env::var("COMPUTERNAME").ok().and_then(trim_non_empty)
    }
}

/// Trim whitespace and drop if empty, so a blank source falls through to the
/// next one.
fn trim_non_empty(value: String) -> Option<String> {
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

pub(crate) fn llm_runtime_config_from_state(state: &AppState) -> LlmRuntimeConfig {
    LlmRuntimeConfig {
        provider: Some(state.agent_config.llm.provider.clone()),
        model: Some(state.agent_config.llm.model.clone()),
        api_base: (!state.agent_config.llm.api_base.trim().is_empty())
            .then(|| state.agent_config.llm.api_base.clone()),
        api_key_env: state.agent_config.llm.api_key_env.clone(),
        api_key: None,
    }
}

/// Build a TUI entry context that carries the active agent role as
/// `runtime_profile_id`. This is shared by the local TUI path (passing
/// `instance_id = None`) and the remote TUI path (passing the remote
/// `base_url` as `instance_id`) so the daemon can resolve `[agent.<name>]`
/// — including `max_turns` — in both modes.
pub(crate) fn tui_entry_context(
    state: &AppState,
    instance_id: Option<String>,
) -> GatewayEntryContext {
    let mut entry = GatewayEntryContext::tui(instance_id);
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

        let entry = tui_entry_context(&state, None);

        assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
    }

    #[test]
    fn tui_entry_context_carries_instance_id_for_remote() {
        let config = Config::default();
        let mut state =
            AppState::new_with_config(&config, PathBuf::from("config.toml"), PathBuf::from("."))
                .expect("app state should initialize");
        state.active_agent_role = Some("plan".to_string());

        let entry = tui_entry_context(&state, Some("http://127.0.0.1:18080".to_string()));

        // Both the role and the remote instance_id must be set so the
        // daemon can resolve [agent.<name>] (max_turns) in remote mode.
        assert_eq!(entry.runtime_profile_id.as_deref(), Some("plan"));
        assert_eq!(entry.instance_id.as_deref(), Some("http://127.0.0.1:18080"));
    }
}
