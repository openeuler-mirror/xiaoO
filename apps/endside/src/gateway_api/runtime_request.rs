use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::unbounded_channel;

use crate::app_state::AppState;
use crate::chat::Message;
use crate::config::Config;
use crate::gateway::{
    AppTurnRequest, GatewayEntryContext, HostedSessionRuntimeConfig, LlmRuntimeConfig,
    SessionOpenRequest, SessionRuntimeDescriptor,
};
use xiaoo_api::chat::AgentId;
use xiaoo_api::chat::{FeatureFlags, ReasoningEffort, TokenBudgetConfig};
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

/// TTL for static fallback and negative results: when dynamic probing is
/// unavailable (missing API key, offline) we fall back to the static value
/// and cache it briefly; expiry automatically retries the dynamic chain, so
/// the process self-corrects within 5 minutes of the key being set or the
/// network returning. Successful dynamic results never expire.
const STATIC_FALLBACK_CACHE_TTL: Duration = Duration::from_secs(5 * 60);

/// How long the turn path waits for the dynamic probe (waited inside a
/// spawned task, so it only delays dispatch and never blocks the event
/// loop; the probe chain's HTTP timeouts sum to ~40s). Past the deadline the
/// static value lets the current turn through while the probe continues in
/// the background and writes the cache (see
/// [`resolve_total_token_budget`]).
const PROBE_INLINE_DEADLINE: Duration = Duration::from_secs(3);

/// Process-wide dynamic context-window cache keyed by
/// `provider|model|api_base`: `build_runtime_config` runs on every local
/// turn and probing can hit the network, so each combination is probed at
/// most once; switching provider / model / api_base changes the key.
fn dynamic_context_window_cache() -> &'static Mutex<HashMap<String, CachedWindow>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedWindow>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Cache entries:
/// - `Dynamic`: probe succeeded, never expires;
/// - `Static`: static fallback, expires per [`STATIC_FALLBACK_CACHE_TTL`]
///   and then the dynamic chain is retried;
/// - `Unresolved`: both the probe and the static estimate failed; also
///   expires per the TTL — while valid, subsequent turns fail fast without
///   touching the network, instead of re-waiting on the probe and reporting
///   the same error every turn.
#[derive(Clone, Copy)]
enum CachedWindow {
    Dynamic { window: usize },
    Static { window: usize, cached_at: Instant },
    Unresolved { cached_at: Instant },
}

impl CachedWindow {
    fn still_valid(&self) -> bool {
        match self {
            Self::Dynamic { .. } => true,
            Self::Static { cached_at, .. } | Self::Unresolved { cached_at } => {
                cached_at.elapsed() < STATIC_FALLBACK_CACHE_TTL
            }
        }
    }

    /// Resolved window on a cache hit; `Unresolved` means the probe and the
    /// static estimate both failed.
    fn resolved_window(&self) -> Option<usize> {
        match self {
            Self::Dynamic { window } | Self::Static { window, .. } => Some(*window),
            Self::Unresolved { .. } => None,
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
}

impl TurnConfigInputs {
    fn active_agent_role_config(&self) -> Option<&crate::config::AgentRoleConfig> {
        self.agent_config
            .active_agent_role_config(self.active_agent_role.as_deref())
    }
}

/// Build a turn's [`HostedSessionRuntimeConfig`] from a submit-time
/// snapshot; failures surface via `SessionTurnUpdate::Err`.
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
    let total_budget = resolve_total_token_budget(&inputs.agent_config)
        .await
        .ok_or_else(|| "invalid TUI runtime state: unable to resolve context window".to_string())?;
    let reserved_for_output = usize::try_from(inputs.agent_config.llm.max_tokens)
        .map_err(|_| "invalid TUI runtime state: invalid [llm].max_tokens".to_string())?;
    let reserved_for_system = DEFAULT_SYSTEM_TOKEN_RESERVE.min(total_budget.saturating_sub(1));

    Ok(HostedSessionRuntimeConfig {
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
    })
}

/// Resolve the session's total token budget: dynamic probing first (waiting
/// only up to [`PROBE_INLINE_DEADLINE`]; past it the probe continues in the
/// background and writes the cache itself), falling back to the static
/// estimate on failure/timeout. Results are cached by
/// `provider|model|api_base`; static and double-failure results are cached
/// briefly so a key being set or the network returning is retried (see
/// [`STATIC_FALLBACK_CACHE_TTL`]).
async fn resolve_total_token_budget(agent_config: &Config) -> Option<usize> {
    resolve_window_with_shared_cache(
        &agent_config.llm,
        crate::config::resolve_context_window(agent_config),
        PROBE_INLINE_DEADLINE,
    )
    .await
}

/// Shared process-wide cache entry point for window resolution: the turn
/// path ([`resolve_total_token_budget`]) and startup validation (main's
/// `resolve_context_window_for_validation`) share one cache key, probe chain
/// and static fallback. The startup probe writes the cache directly, so the
/// first turn hits it and skips the repeat wait.
pub(crate) async fn resolve_window_with_shared_cache(
    llm: &crate::config::LlmConfig,
    static_window: Option<usize>,
    inline_deadline: Duration,
) -> Option<usize> {
    let api_base = (!llm.api_base.trim().is_empty()).then(|| llm.api_base.clone());
    let provider = llm.provider.clone();
    let model = llm.model.clone();
    let api_key_env = llm.api_key_env.clone();
    resolve_window_with_probe(
        format!("{}|{}|{}", llm.provider, llm.model, llm.api_base),
        static_window,
        async move {
            probe_dynamic_context_window(&provider, &model, None, api_key_env, api_base).await
        },
        inline_deadline,
    )
    .await
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
/// Ollama `/api/show`). Shared by the CLI and TUI gateways; any failure
/// just logs a warn and returns `None`, leaving the caller to its own
/// static estimate.
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

/// State-machine core of [`resolve_total_token_budget`] (extracted as a
/// free function so unit tests can inject a synthetic probe and a short
/// deadline without touching the network):
///
/// 1. A valid cache hit (`Dynamic` forever, `Static`/`Unresolved` within the
///    TTL) returns immediately;
/// 2. the probe is spawned into a background task that **writes the
///    `Dynamic` cache itself on success** — after the turn side gives up
///    waiting, the task still completes and corrects the cache, so slow
///    endpoints lose no dynamic accuracy;
/// 3. the turn side waits inline only up to `inline_deadline`: finishing
///    within it uses the dynamic value;
/// 4. timeout/failure falls back to the static value: before writing under
///    the lock, check whether the background task has already written
///    `Dynamic` (yield to it, never overwrite); if the static estimate also
///    fails, write `Unresolved` so subsequent turns fail fast within the TTL
///    without touching the network.
async fn resolve_window_with_probe(
    cache_key: String,
    static_window: Option<usize>,
    probe: impl Future<Output = Option<usize>> + Send + 'static,
    inline_deadline: Duration,
) -> Option<usize> {
    if let Some(cached) = dynamic_context_window_cache()
        .lock()
        .ok()
        .and_then(|guard| guard.get(&cache_key).copied())
        .filter(|cached| cached.still_valid())
    {
        return cached.resolved_window();
    }

    let probe_key = cache_key.clone();
    let probe = tokio::spawn(async move {
        let window = probe.await;
        if let Some(window) = window {
            if let Ok(mut guard) = dynamic_context_window_cache().lock() {
                guard.insert(probe_key, CachedWindow::Dynamic { window });
            }
        }
        window
    });

    // Ok(Ok(Some(..))): finished within the deadline, cache already written
    // by the task;
    // Ok(Ok(None)) / Ok(Err(_)): probe failed fast / task panicked, nothing
    // keeps running in the background;
    // Err(_): the turn side timed out — the JoinHandle is dropped, the
    // detached task keeps writing the cache in the background.
    match tokio::time::timeout(inline_deadline, probe).await {
        Ok(Ok(Some(window))) => return Some(window),
        Ok(Ok(None)) | Ok(Err(_)) => {}
        Err(_) => {
            tracing::debug!(
                key = %cache_key,
                "dynamic context window probe exceeded inline deadline; \
                 using static estimate for this turn (probe continues in background)"
            );
        }
    }

    let Ok(mut guard) = dynamic_context_window_cache().lock() else {
        // Poisoned lock: skip the cache, return the static value directly.
        return static_window;
    };
    // The background probe may write `Dynamic` between our timeout and this
    // lock acquisition — yield to it, never overwrite.
    if let Some(CachedWindow::Dynamic { window }) = guard.get(&cache_key) {
        return Some(*window);
    }
    match static_window {
        Some(window) => {
            guard.insert(
                cache_key,
                CachedWindow::Static {
                    window,
                    cached_at: Instant::now(),
                },
            );
            Some(window)
        }
        None => {
            guard.insert(
                cache_key,
                CachedWindow::Unresolved {
                    cached_at: Instant::now(),
                },
            );
            None
        }
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
