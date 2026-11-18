pub mod attach;
pub mod config;
mod deadline;
pub mod entry;
mod json_events;
pub mod skills;

use std::sync::{Arc, Mutex};

use serde_json::Value;
use xiaoo_api::chat::HookerRegistryConfig;
use xiaoo_api::chat::ReasoningEffort;
use xiaoo_api::events::{LoopEndSummary, ToolResultEvent};
use xiaoo_api::llm::CompressionPipeline;
use xiaoo_api::llm::{
    build_context_manager, create_llm_provider, CompactOverrides, LlmProviderConfig,
    LlmProviderWrapper,
};
use xiaoo_shared::gateway::llm_assembly::{list_llm_models, LlmAssemblyInput};
// ---------------------------------------------------------------------------
// CliEventSink
// ---------------------------------------------------------------------------

pub struct CliEventSink {
    state: Mutex<CliEventSinkState>,
}

#[derive(Default)]
struct CliEventSinkState {
    active_assistant_agent: Option<String>,
    last_assistant_snapshot_len: usize,
}

impl CliEventSink {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(CliEventSinkState::default()),
        }
    }

    fn finish_assistant_line(&self) {
        let mut state = self
            .state
            .lock()
            .expect("cli event sink state mutex should not be poisoned");
        if state.active_assistant_agent.is_some() {
            eprintln!();
        }
        state.active_assistant_agent = None;
        state.last_assistant_snapshot_len = 0;
    }
}

impl xiaoo_api::events::LoopEventSink for CliEventSink {
    fn on_turn_start(&self, agent_id: &xiaoo_api::chat::AgentId, turn: u32) {
        self.finish_assistant_line();
        eprintln!("--- turn {} [{}] ---", turn, agent_id.0);
    }

    fn on_assistant_message(&self, agent_id: &xiaoo_api::chat::AgentId, text: &str) {
        let mut state = self
            .state
            .lock()
            .expect("cli event sink state mutex should not be poisoned");
        let is_same_stream = state
            .active_assistant_agent
            .as_deref()
            .is_some_and(|active| active == agent_id.0);
        let prev_len = if is_same_stream {
            state.last_assistant_snapshot_len
        } else {
            if state.active_assistant_agent.is_some() {
                eprintln!();
            }
            0
        };

        if prev_len >= text.len() || !text.is_char_boundary(prev_len) {
            return;
        }

        if !is_same_stream {
            eprint!("[assistant: {}] ", agent_id.0);
            state.active_assistant_agent = Some(agent_id.0.clone());
        }

        eprint!("{}", &text[prev_len..]);
        state.last_assistant_snapshot_len = text.len();
    }

    fn on_tool_result(&self, agent_id: &xiaoo_api::chat::AgentId, event: &ToolResultEvent) {
        self.finish_assistant_line();
        let status = if event.is_error { "ERR" } else { "OK" };
        eprintln!(
            "  [tool: {}] {} ({}) => {}",
            agent_id.0, event.tool_name, status, event.output_preview
        );
    }

    fn on_loop_end(&self, agent_id: &xiaoo_api::chat::AgentId, summary: &LoopEndSummary) {
        self.finish_assistant_line();
        eprintln!(
            "--- end [{}] (turns={}, tokens={}, reason={}) ---",
            agent_id.0, summary.turn_count, summary.total_tokens, summary.stop_reason
        );
    }
}

// ---------------------------------------------------------------------------
// CLI config (merged from file + CLI args)
// ---------------------------------------------------------------------------

pub struct CliConfig {
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub api_base: Option<String>,
    pub request_headers: xiaoo_api::llm::RequestHeadersConfig,
    pub trace: Value,
    pub system_prompt: String,
    pub max_turns: u32,
    pub context_limit_tokens: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub enable_tools: bool,
    pub visible_tools: Option<Vec<String>>,
    pub reasoning_effort: ReasoningEffort,
    pub kvcache_enabled: bool,
    pub kvcache_debug_enabled: bool,
    pub compact: config::CompactSection,
    pub hooker: HookerRegistryConfig,
    pub operation_backend: Option<crate::backend::GatewayBackendConfig>,
    pub skills_config: xiaoo_shared::skills_support::SkillsConfig,
    pub subagent: std::collections::BTreeMap<String, config::SubagentRoleConfig>,
    pub mcp_servers: Vec<xiaoo_shared::mcp_support::McpServerConfig>,
    pub memory_automation: xiaoo_shared::gateway::MemoryAutomationConfig,
}

// ---------------------------------------------------------------------------
// Helpers for CLI gateway integration
// ---------------------------------------------------------------------------

pub fn build_llm_provider(
    config: &CliConfig,
    agent_id: Option<String>,
) -> Result<Arc<LlmProviderWrapper>, Box<dyn std::error::Error>> {
    let llm_config: LlmProviderConfig = {
        let mut c = LlmProviderConfig::new(&config.provider, &config.model)
            .with_request_headers(config.request_headers.clone());
        if let Some(ref key) = config.api_key {
            c = c.with_api_key(key);
        }
        if let Some(ref base) = config.api_base {
            c = c.with_api_base(base);
        }
        c
    };
    let mut provider = create_llm_provider(&llm_config, agent_id, None)?;
    if let Some(limit) = config.max_output_tokens {
        provider = provider.with_max_output_tokens(limit, config.reasoning_effort)?;
    }
    Ok(Arc::new(provider))
}

pub fn build_compression_pipeline(
    config: &CliConfig,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> Result<Arc<dyn CompressionPipeline>, Box<dyn std::error::Error>> {
    // Route through the shared `xiaoo_api::llm::build_context_manager` so the local
    // CLI and the daemon use the *same* defaults and estimator tuning. A
    // missing `[compact]` section yields all-`None` overrides, which the
    // helper layers onto its built-in defaults — identical to the daemon.
    let cc = &config.compact;
    let overrides = CompactOverrides {
        auto_compact_ratio: cc.auto_compact_ratio,
        blocking_ratio: cc.blocking_ratio,
        summary_max_tokens: cc.summary_max_tokens,
        summary_preserve_tail: cc.summary_preserve_tail,
        summary_llm_max_tokens: cc.summary_llm_max_tokens,
    };
    let pipeline = build_context_manager(Some(&overrides), Arc::clone(llm_provider))
        .map_err(|e| format!("context manager: {e}"))?;
    Ok(pipeline)
}

/// CLI context-window resolution: same shared entry point as the TUI turn
/// path (single-flight cache, bounded probe chain), so a cold invocation
/// returns within a bounded wait even when endpoints are slow or unreachable.
pub async fn resolve_effective_context_window(
    config: &CliConfig,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> Result<usize, String> {
    let static_window = llm_provider.capabilities().max_context_window.max(1);
    let llm = crate::config::LlmConfig {
        provider: config.provider.clone(),
        model: config.model.clone(),
        api_key_env: config.api_key_env.clone(),
        api_base: config.api_base.clone().unwrap_or_default(),
        ..Default::default()
    };
    let window = crate::gateway_api::runtime_request::resolve_window_with_shared_cache(
        &llm,
        config.api_key.clone(),
        config
            .context_limit_tokens
            .is_none()
            .then_some(static_window),
        crate::gateway_api::runtime_request::PROBE_TOTAL_BUDGET,
    )
    .await;
    if let Some(limit) = config.context_limit_tokens.map(|value| value as usize) {
        if let Some(maximum) = window.filter(|maximum| limit > *maximum) {
            return Err(format!(
                "--context-window {limit} exceeds the model context limit of {maximum}"
            ));
        }
        return Ok(limit);
    }
    Ok(window.unwrap_or(static_window))
}

pub async fn validate_model_output_limit(config: &CliConfig) -> Result<(), String> {
    let Some(limit) = config.max_output_tokens else {
        return Ok(());
    };
    let input = LlmAssemblyInput {
        provider: config.provider.clone(),
        model: config.model.clone(),
        api_base: config.api_base.clone(),
        api_key: config.api_key.clone(),
        api_key_env: config.api_key_env.clone(),
        request_headers: config.request_headers.clone(),
        context_window_override: None,
        agent_id: None,
    };
    // Catalog metadata is optional. Bound this lookup independently so
    // custom endpoints without a catalog can still run.
    let models =
        tokio::time::timeout(std::time::Duration::from_secs(2), list_llm_models(input)).await;
    if let Ok(Ok(models)) = models {
        let model_id = config.model.trim();
        let model_id = model_id.strip_prefix("models/").unwrap_or(model_id);
        let maximum = models
            .iter()
            .find(|model| model.id.eq_ignore_ascii_case(model_id))
            .and_then(|model| model.max_output_tokens)
            .filter(|maximum| *maximum > 0 && u64::from(limit) > *maximum);
        if let Some(maximum) = maximum {
            return Err(format!(
                "--max-tokens {limit} exceeds the model output limit of {maximum}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../tests/unit/endside/cli_test.rs"]
mod tests;
