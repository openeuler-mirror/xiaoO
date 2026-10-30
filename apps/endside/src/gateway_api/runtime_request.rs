use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc::unbounded_channel, watch};

use crate::app_state::AppState;
use crate::chat::Message;
use crate::config::Config;
use crate::gateway::{
    AppTurnRequest, GatewayEntryContext, HostedSessionRuntimeConfig, LlmRuntimeConfig,
    SessionOpenRequest, SessionRuntimeDescriptor,
};
use xiaoo_api::chat::AgentId;
use xiaoo_api::chat::{
    validate_max_tokens_against_window, FeatureFlags, ReasoningEffort, TokenBudgetConfig,
};
use xiaoo_shared::gateway::tool_assembly::{discover_tool_names, ToolAssemblyInput};

use super::runtime::GatewayRuntime;
use xiaoo_api::runtime::spawn_prefetch;

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

/// TTL for negative probe results: while valid, later resolutions reuse
/// the conclusion without touching the network; expiry retries the chain,
/// so a key appearing or the network returning self-corrects within this
/// TTL. Positive results never expire.
const NEGATIVE_RESULT_TTL: Duration = Duration::from_secs(5 * 60);

/// Total budget for the off-path resolutions (startup warm-up and CLI). The
/// probe chain's per-stage budgets sum to ~7s, so 8s lets the chain
/// self-terminate instead of being cut off mid-chain.
pub(crate) const PROBE_TOTAL_BUDGET: Duration = Duration::from_secs(8);

/// Turn-path budget: the interactive path must not block for the full chain
/// budget. A probe that outlives it keeps running in the background (see
/// [`resolve_window_with_probe`]) and corrects the cache; this turn uses the
/// static estimate.
const PROBE_TURN_BUDGET: Duration = Duration::from_secs(3);

/// Process-wide dynamic context-window cache keyed by provider, model,
/// api_base and credential fingerprint: `build_runtime_config` runs on every
/// local turn and probing can hit the network, so each combination carries at
/// most one in-flight probe (see [`WindowCacheSlot`]).
fn dynamic_context_window_cache() -> &'static Mutex<HashMap<String, WindowCacheSlot>> {
    static CACHE: OnceLock<Mutex<HashMap<String, WindowCacheSlot>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// One probe's conclusion entry: `window = Some(_)` is the authoritative
/// probed window; `window = None` is a negative result (the chain ran to
/// completion without a hit).
///
/// Static fallback values are NOT cached — callers supply them per call,
/// and the negative entry takes over the "don't re-probe within the TTL"
/// role.
#[derive(Clone, Copy)]
struct CachedWindow {
    window: Option<usize>,
    resolved_at: Instant,
}

impl CachedWindow {
    fn negative() -> Self {
        Self {
            window: None,
            resolved_at: Instant::now(),
        }
    }

    fn still_valid(&self) -> bool {
        match self.window {
            Some(_) => true,
            None => self.resolved_at.elapsed() < NEGATIVE_RESULT_TTL,
        }
    }
}

/// Cache-table slot: a probe in flight (the merge point) or its conclusion.
enum WindowCacheSlot {
    /// Probe task running: concurrent callers clone the receiver under the
    /// lock and await its conclusion outside it (single-flight). Channel
    /// write/send ordering is the invariant documented on [`run_probe`].
    InFlight {
        rx: watch::Receiver<Option<usize>>,
    },
    Resolved(CachedWindow),
}

/// Window-resolution outcome for the turn path, separating probed values
/// from static-family estimates:
///
/// - `Trusted`: a probed value (cache hit, or a probe that concluded within
///   the caller's budget) — authoritative enough to reject a turn whose
///   `max_tokens` leaves no input space.
/// - `Estimated`: the protocol-family static fallback. It never rejects a
///   turn: the estimate can undershoot the real window, and rejecting on
///   it would block a working config whenever probing is offline or slow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResolvedWindow {
    Trusted { window: usize },
    Estimated { window: usize },
}

impl ResolvedWindow {
    pub(crate) fn window(self) -> usize {
        match self {
            Self::Trusted { window } | Self::Estimated { window } => window,
        }
    }
}

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
        command_context: xiaoo_api::chat::CommandContext,
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
        command_context: Option<xiaoo_api::chat::CommandContext>,
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
                let has_api_key = std::env::var(trimmed)
                    .map(|v| !v.trim().is_empty())
                    .unwrap_or(false);
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
        self.sync_session_messages_from_store(state).await;
        let open_request = self.session_open_request(state)?;
        let turn_request =
            self.turn_request(state, prompt.clone(), command_context, chain_depth)?;

        // UI feedback first: the config build (context-window probe) runs in
        // the spawned task below so Enter never blocks the event loop; build
        // failures surface through the `SessionTurnUpdate::Err` channel.
        state.chat_state.stick_to_bottom = true;
        // Capture the turn generation: the task may only record its dispatch
        // timestamp while this generation is current — cancel, reset, or a
        // newer submit revokes it.
        let turn_generation = self
            .turn_clock
            .lock()
            .ok()
            .map(|mut turn_clock| turn_clock.invalidate())
            .unwrap_or(0);
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
        // A previous turn may still be draining after an Esc cancel; its
        // receiver is replaced below, so make sure the new turn's updates
        // are processed normally (not swallowed by the drain guard).
        self.draining_after_cancel = false;

        let (updates_tx, updates_rx) = unbounded_channel();
        let (interaction_tx, interaction_rx) = unbounded_channel();
        self.interaction_reply_tx = Some(interaction_tx);
        self.stream_rx = Some(updates_rx);
        let cancel_token = tokio_util::sync::CancellationToken::new();
        self.cancel_token = Some(cancel_token.clone());

        let session_gateway = self.session_gateway.clone();
        let pending_user_messages = self.pending_user_messages.clone();
        let prefetch_session_id = state.session_id.clone();
        let turn_clock = self.turn_clock.clone();
        let config_inputs = TurnConfigInputs {
            agent_config: state.agent_config.clone(),
            workspace: state.workspace.clone(),
            active_agent_role: state.active_agent_role.clone(),
            reasoning_effort: state.reasoning_effort,
            config_path: state.config_path.clone(),
        };
        tokio::spawn(async move {
            let runtime_config = match build_runtime_config_from_inputs(config_inputs).await {
                Ok(config) => config,
                Err(error) => {
                    let _ = updates_tx.send(crate::session_gateway::SessionTurnUpdate::Err(error));
                    return;
                }
            };
            // Esc during the build: a cancelled turn must not be dispatched;
            // dropping `updates_tx` lets the drain guard settle quietly.
            if cancel_token.is_cancelled() {
                return;
            }
            // Dispatch timestamp: after the build (probe excluded), before
            // the open RPC; generation-guarded against stale tasks.
            if let Ok(mut clock) = turn_clock.lock() {
                clock.record_dispatch(turn_generation, Instant::now());
            }
            if runtime_config.descriptor.feature_flags.kvcache_enabled {
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
            // The open RPC can span an Esc press; re-check before dispatch
            // (the token passed into `spawn_turn` is the final backstop).
            if cancel_token.is_cancelled() {
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
}

/// Submit-time state snapshot for building a turn's runtime config in the
/// background task; the config is shared via `Arc` (handle, not deep copy).
pub(crate) struct TurnConfigInputs {
    agent_config: Arc<Config>,
    workspace: std::path::PathBuf,
    active_agent_role: Option<String>,
    reasoning_effort: ReasoningEffort,
    /// Config path for turn-time error messages that point the user at the
    /// `[llm]` section to fix.
    config_path: std::path::PathBuf,
}

impl TurnConfigInputs {
    fn active_agent_role_config(&self) -> Option<&crate::config::AgentRoleConfig> {
        self.agent_config
            .active_agent_role_config(self.active_agent_role.as_deref())
    }
}

/// Build a turn's [`HostedSessionRuntimeConfig`] from a submit-time
/// snapshot; fatal failures (a `max_tokens` that leaves no input space on a
/// probed window — see [`evaluate_max_tokens_against_window`] — or an
/// outright unresolvable window — see [`unresolved_window_error`]) surface
/// via `SessionTurnUpdate::Err`.
async fn build_runtime_config_from_inputs(
    inputs: TurnConfigInputs,
) -> Result<HostedSessionRuntimeConfig, String> {
    let agent_id = resolve_agent_id(None, None, &inputs.agent_config)?;
    let role_config = inputs.active_agent_role_config();
    let system_prompt = role_config
        .and_then(|role| role.prompt.clone())
        .unwrap_or_else(|| {
            DEFAULT_SYSTEM_PROMPT
                .trim_end_matches(['\r', '\n'])
                .to_string()
        });
    let resolved_window = resolve_window_for_turn(&inputs.agent_config)
        .await
        .ok_or_else(|| {
            unresolved_window_error(
                &inputs.agent_config.llm.provider,
                &inputs.agent_config.llm.model,
                &inputs.config_path,
            )
        })?;
    evaluate_max_tokens_against_window(
        resolved_window,
        inputs.agent_config.llm.max_tokens,
        &inputs.agent_config.llm.model,
        &inputs.config_path,
    )?;
    let total_budget = resolved_window.window();
    let reserved_for_output = usize::try_from(inputs.agent_config.llm.max_tokens)
        .map_err(|_| "invalid TUI runtime state: invalid [llm].max_tokens".to_string())?;
    let reserved_for_system = DEFAULT_SYSTEM_TOKEN_RESERVE.min(total_budget.saturating_sub(1));

    let runtime_config = HostedSessionRuntimeConfig {
        descriptor: SessionRuntimeDescriptor {
            agent_id: AgentId(agent_id),
            model: inputs.agent_config.llm.model.clone(),
            llm: Some(llm_runtime_config_from_parts(
                &inputs.agent_config,
                inputs.reasoning_effort,
            )),
            system_prompt,
            feature_flags: FeatureFlags {
                kvcache_enabled: inputs.agent_config.llm.kvcache_enabled,
                kvcache_debug_enabled: inputs.agent_config.llm.kvcache_debug_enabled,
                redact_secrets_display: inputs.agent_config.tui.redact_secrets_display,
                ..Default::default()
            },
            token_budget: TokenBudgetConfig {
                total_budget,
                reserved_for_output,
                reserved_for_system,
                hard_limit_ratio: 1.0,
            },
            workspace_root: inputs.workspace.clone(),
            max_turns: role_config.and_then(|role| role.max_turns),
            subagent_roles: inputs
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
        provider: inputs.agent_config.llm.provider.clone(),
        model: inputs.agent_config.llm.model.clone(),
        api_key: None,
        api_key_env: inputs.agent_config.llm.api_key_env.clone(),
        api_base: if inputs.agent_config.llm.api_base.trim().is_empty() {
            None
        } else {
            Some(inputs.agent_config.llm.api_base.clone())
        },
        visible_tool_names: resolve_visible_tool_names(
            &inputs.agent_config,
            inputs.active_agent_role.as_deref(),
            &inputs.workspace,
        )
        .await,
        compression_pipeline: None,
        llm_provider: None,
        trace: inputs
            .agent_config
            .trace
            .clone()
            .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
        hooker: inputs.agent_config.hooker.clone(),
        operation_backend: inputs.agent_config.operation_backend.clone(),
        lsp_registry: inputs.agent_config.build_lsp_registry(),
        skills_config: inputs.agent_config.resolve_skills_config(),
        subagent_roles: inputs
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
        mcp_servers: inputs.agent_config.mcp_servers().to_vec(),
        memory_automation: inputs.agent_config.memory_automation.clone(),
    };

    Ok(runtime_config)
}

/// Resolve the turn's context window with trust semantics (see
/// [`ResolvedWindow`]): dynamic probing bounded by [`PROBE_TURN_BUDGET`],
/// falling back to the static estimate. Caching and retry semantics live
/// in [`resolve_window_with_probe`].
async fn resolve_window_for_turn(agent_config: &Config) -> Option<ResolvedWindow> {
    resolve_window_with_shared_cache(
        &agent_config.llm,
        None,
        crate::config::resolve_context_window(agent_config),
        PROBE_TURN_BUDGET,
    )
    .await
}

/// Startup cache warm-up: resolves the dynamic context window off the
/// startup critical path (the TUI renders immediately) so the first turn
/// usually hits the shared cache instead of probing inline.
pub(crate) async fn warm_context_window_cache(
    llm: crate::config::LlmConfig,
    static_window: Option<usize>,
) {
    let _ = resolve_window_with_shared_cache(&llm, None, static_window, PROBE_TOTAL_BUDGET).await;
}

/// Turn-time `max_tokens` gate — the single enforcement point for the
/// window check: every dispatch funnels through
/// [`build_runtime_config_from_inputs`], including provider/model switches
/// made at runtime via `/connect`.
///
/// A probed ([`ResolvedWindow::Trusted`]) window is authoritative: a
/// `max_tokens` at or above it leaves no input space, so the turn is
/// rejected before any RPC or API call. The rejection text comes from the
/// shared `xiaoo_api::chat::validate_max_tokens_against_window` — the same
/// single source the daemon's startup validation uses, so the two
/// enforcement points cannot drift apart. A static estimate never rejects:
/// it can undershoot the real window (offline probing + a large-window
/// model), and rejecting on it would block a working config.
fn evaluate_max_tokens_against_window(
    resolved: ResolvedWindow,
    max_tokens: u32,
    model: &str,
    config_path: &std::path::Path,
) -> Result<(), String> {
    match resolved {
        ResolvedWindow::Trusted { window } => validate_max_tokens_against_window(
            max_tokens as usize,
            window,
            model,
            config_path,
            "[llm]",
        ),
        ResolvedWindow::Estimated { .. } => Ok(()),
    }
}

/// Turn-path error for the outright window-resolution failure (probe
/// returned nothing and no static estimate exists — see
/// [`resolve_window_for_turn`]). In practice an unrecognized provider name
/// is the only cause: known providers always carry a protocol-family
/// estimate and take the [`ResolvedWindow::Estimated`] fallback instead.
/// The message therefore points the user at the provider name rather than
/// blaming internal state. The failure is cached as a negative
/// [`CachedWindow`] entry and retried automatically once
/// [`NEGATIVE_RESULT_TTL`] expires, so no retry hint is needed in the text.
fn unresolved_window_error(provider: &str, model: &str, config_path: &std::path::Path) -> String {
    format!(
        "unable to resolve the context window for model {} (provider {}): the dynamic probe \
         failed and no static window estimate exists for this provider name.\n\
         Configuration file: {}\n\
         Suggestions:\n\
           - Check the provider name in [llm] section (use a recognized provider name, or a \
         protocol family such as \"openai-compatible\" with api_base)",
        model,
        provider,
        config_path.display(),
    )
}

/// Shared process-wide cache entry point for window resolution: the turn
/// path ([`resolve_window_for_turn`]), the startup warm-up
/// ([`warm_context_window_cache`]) and the CLI
/// (`cli::resolve_effective_context_window`) share one cache table, probe
/// chain, total budget and static fallback.
pub(crate) async fn resolve_window_with_shared_cache(
    llm: &crate::config::LlmConfig,
    api_key: Option<String>,
    static_window: Option<usize>,
    budget: Duration,
) -> Option<ResolvedWindow> {
    let api_base = (!llm.api_base.trim().is_empty()).then(|| llm.api_base.clone());
    let provider = llm.provider.clone();
    let model = llm.model.clone();
    let api_key_env = llm.api_key_env.clone();
    let cache_key = format!(
        "{}|{}|{}|{:016x}",
        llm.provider,
        llm.model,
        llm.api_base,
        credential_fingerprint(&api_key, &api_key_env)
    );
    resolve_window_with_probe(
        cache_key,
        static_window,
        async move {
            probe_dynamic_context_window(&provider, &model, api_key, api_key_env, api_base).await
        },
        budget,
    )
    .await
}

/// Distinguishes credential sources in the cache key: a negative result from
/// one source (e.g. a keyless TUI probe) must not suppress probing from
/// another (e.g. the CLI's explicit key) within the TTL.
fn credential_fingerprint(api_key: &Option<String>, api_key_env: &Option<String>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    api_key.hash(&mut hasher);
    api_key_env.hash(&mut hasher);
    hasher.finish()
}

impl GatewayRuntime {
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
        command_context: Option<xiaoo_api::chat::CommandContext>,
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
            reasoning_effort: Some(state.reasoning_effort),
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
/// unresolved — purely informational. Resolution lives in the shared
/// `host_identity` module so the lease display and the same-host check
/// cannot drift apart.
pub(crate) fn hostname() -> Option<String> {
    xiaoo_shared::host_identity::local_hostname()
}

pub(crate) fn llm_runtime_config_from_state(state: &AppState) -> LlmRuntimeConfig {
    llm_runtime_config_from_parts(&state.agent_config, state.reasoning_effort)
}

pub(crate) fn llm_runtime_config_from_parts(
    agent_config: &Config,
    reasoning_effort: ReasoningEffort,
) -> LlmRuntimeConfig {
    LlmRuntimeConfig {
        profile_id: None,
        provider: Some(agent_config.llm.provider.clone()),
        model: Some(agent_config.llm.model.clone()),
        api_base: (!agent_config.llm.api_base.trim().is_empty())
            .then(|| agent_config.llm.api_base.clone()),
        api_key_env: agent_config.llm.api_key_env.clone(),
        api_key: None,
        reasoning_effort: Some(reasoning_effort),
    }
}

/// Dynamic context-window probing: builds a `ResolvedConfig` and goes
/// through the shared resolution chain (provider catalog > OpenRouter >
/// Ollama `/api/show`). Reached through [`resolve_window_with_shared_cache`]
/// by every caller (turn path, warm-up, CLI); any failure just logs a warn
/// and returns `None`, leaving the caller to its own static estimate.
pub(crate) async fn probe_dynamic_context_window(
    provider: &str,
    model: &str,
    api_key: Option<String>,
    api_key_env: Option<String>,
    api_base: Option<String>,
) -> Option<usize> {
    use xiaoo_api::llm::{resolve_config, resolve_model_context_length, ResolveInput};

    if provider.trim().is_empty() {
        return None;
    }
    let input = ResolveInput {
        provider: Some(provider.to_string()),
        protocol: None,
        api_key,
        api_key_env,
        base_url: api_base,
    };
    let resolved = match resolve_config(input) {
        Ok(resolved) => resolved,
        Err(error) => {
            tracing::warn!(
                provider = %provider,
                model = %model,
                %error,
                "failed to resolve provider config for dynamic context window; falling back"
            );
            return None;
        }
    };
    match resolve_model_context_length(&resolved, model).await {
        Ok(Some(context)) => usize::try_from(context).ok().filter(|value| *value > 0),
        Ok(None) => None,
        Err(error) => {
            tracing::warn!(
                provider = %provider,
                model = %model,
                %error,
                "dynamic context window probe failed; falling back"
            );
            None
        }
    }
}

/// State-machine core of [`resolve_window_for_turn`] (free function so unit
/// tests can inject a synthetic probe and budget without the network):
///
/// - a valid conclusion returns immediately (positive → `Trusted`, negative →
///   static estimate); an in-flight slot's receiver is cloned to merge onto
///   the existing probe; otherwise a fresh probe task is registered;
/// - the waiter awaits the conclusion for at most `budget`. On timeout it
///   falls back to the static estimate and writes **no cache entry** (the
///   probe is still running, so writing would misrecord it as failed); the
///   detached task records the real conclusion when it finishes;
/// - a task that dies without sending routes waiters into the dead-probe
///   branch of [`wait_for_probe_conclusion`], which records a negative entry.
async fn resolve_window_with_probe(
    cache_key: String,
    static_window: Option<usize>,
    probe: impl Future<Output = Option<usize>> + Send + 'static,
    budget: Duration,
) -> Option<ResolvedWindow> {
    let rx = {
        let Ok(mut guard) = dynamic_context_window_cache().lock() else {
            // Poisoned lock: neither reading nor updating the cache is
            // possible — degrade to an uncached probe, still budget-bound.
            return match tokio::time::timeout(budget, probe).await {
                Ok(Some(window)) => Some(ResolvedWindow::Trusted { window }),
                _ => estimate(static_window),
            };
        };
        match guard.get(&cache_key) {
            Some(WindowCacheSlot::Resolved(entry)) if entry.still_valid() => {
                return cached_outcome(*entry, static_window);
            }
            Some(WindowCacheSlot::InFlight { rx }) => rx.clone(),
            // Expired negative entry or missing slot: register a fresh
            // probe (the insert overwrites the expired entry). The map
            // keeps a receiver for later mergers.
            _ => {
                let (done, rx) = watch::channel(None);
                guard.insert(
                    cache_key.clone(),
                    WindowCacheSlot::InFlight { rx: rx.clone() },
                );
                let probe_key = cache_key.clone();
                tokio::spawn(run_probe(probe_key, probe, done));
                rx
            }
        }
    };
    wait_for_probe_conclusion(rx, cache_key, static_window, budget).await
}

/// Probe task body: writes the conclusion before sending so the cache holds
/// it by the time a waiter wakes. `done` is the channel's only sender, so a
/// task that dies without sending drops it, closing the channel and routing
/// waiters into the dead-probe branch instead of pinning them on a dead slot.
async fn run_probe(
    cache_key: String,
    probe: impl Future<Output = Option<usize>>,
    done: watch::Sender<Option<usize>>,
) {
    let window = probe.await;
    let entry = CachedWindow {
        window,
        resolved_at: Instant::now(),
    };
    if let Ok(mut guard) = dynamic_context_window_cache().lock() {
        guard.insert(cache_key, WindowCacheSlot::Resolved(entry));
    }
    let _ = done.send(window);
}

/// Waits for an in-flight probe's conclusion outside the lock (the await
/// side of [`resolve_window_with_probe`]).
async fn wait_for_probe_conclusion(
    mut rx: watch::Receiver<Option<usize>>,
    cache_key: String,
    static_window: Option<usize>,
    budget: Duration,
) -> Option<ResolvedWindow> {
    match tokio::time::timeout(budget, rx.changed()).await {
        Ok(Ok(())) => match *rx.borrow_and_update() {
            Some(window) => Some(ResolvedWindow::Trusted { window }),
            None => estimate(static_window),
        },
        // Every sender dropped = the probe task died without sending. The
        // real conclusion may already have been written before it died
        // (write precedes send), so re-read the slot once; if it is still
        // in-flight, record a negative entry.
        Ok(Err(_)) => {
            if let Ok(mut guard) = dynamic_context_window_cache().lock() {
                match guard.get(&cache_key) {
                    Some(WindowCacheSlot::Resolved(entry)) => {
                        return cached_outcome(*entry, static_window);
                    }
                    _ => {
                        guard.insert(
                            cache_key,
                            WindowCacheSlot::Resolved(CachedWindow::negative()),
                        );
                    }
                }
            }
            estimate(static_window)
        }
        Err(_) => {
            tracing::debug!(
                key = %cache_key,
                "dynamic context window probe exceeded total budget; \
                 using static estimate for this turn (probe continues in background)"
            );
            estimate(static_window)
        }
    }
}

/// Maps a cache conclusion to turn-window semantics: positive → `Trusted`;
/// negative → the caller-supplied static estimate.
fn cached_outcome(entry: CachedWindow, static_window: Option<usize>) -> Option<ResolvedWindow> {
    match entry.window {
        Some(window) => Some(ResolvedWindow::Trusted { window }),
        None => estimate(static_window),
    }
}

/// The caller-supplied static estimate: the fallback whenever probing
/// yields no conclusion (negative result / budget exhausted / task death).
fn estimate(static_window: Option<usize>) -> Option<ResolvedWindow> {
    static_window.map(|window| ResolvedWindow::Estimated { window })
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

async fn resolve_visible_tool_names(
    agent_config: &Config,
    active_agent_role: Option<&str>,
    workspace: &std::path::Path,
) -> Option<Vec<String>> {
    let role = active_agent_role.and_then(|role_id| agent_config.agent_role(role_id))?;
    if role.tools.is_empty() {
        return None;
    }

    let input = ToolAssemblyInput {
        workspace_root: Some(workspace.to_path_buf()),
        ..ToolAssemblyInput::default()
    };
    let all_tool_names: BTreeSet<String> = discover_tool_names(&input)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|name| name.0)
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
#[path = "../../../../tests/unit/endside/gateway_api/runtime_request_test.rs"]
mod tests;
