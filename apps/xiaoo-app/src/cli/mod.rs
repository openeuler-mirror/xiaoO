pub mod config;

use std::sync::{Arc, Mutex};

use agent_contracts::CompressionPipeline;
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use agent_types::hook::HookerRegistryConfig;
use agent_types::{CompletionConfig, ReasoningEffort};
use compact::{
    ContextManager, ContextManagerConfig, ContextThresholds, MicroCompactionPolicy,
    RoughTokenEstimator, RoughTokenEstimatorConfig, SummaryCompressionBudget,
};
use llm_client::{
    create_llm_provider, resolve_config, resolve_model_context_length, LlmProviderConfig,
    LlmProviderWrapper, ResolveInput,
};
use serde_json::Value;

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

impl agent_contracts::events::LoopEventSink for CliEventSink {
    fn on_turn_start(&self, agent_id: &agent_types::common::ids::AgentId, turn: u32) {
        self.finish_assistant_line();
        eprintln!("--- turn {} [{}] ---", turn, agent_id.0);
    }

    fn on_assistant_message(&self, agent_id: &agent_types::common::ids::AgentId, text: &str) {
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

    fn on_tool_result(
        &self,
        agent_id: &agent_types::common::ids::AgentId,
        event: &ToolResultEvent,
    ) {
        self.finish_assistant_line();
        let status = if event.is_error { "ERR" } else { "OK" };
        eprintln!(
            "  [tool: {}] {} ({}) => {}",
            agent_id.0, event.tool_name, status, event.output_preview
        );
    }

    fn on_loop_end(&self, agent_id: &agent_types::common::ids::AgentId, summary: &LoopEndSummary) {
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
    pub trace: Value,
    pub system_prompt: String,
    pub max_turns: u32,
    pub enable_tools: bool,
    pub context_window: Option<usize>,
    pub reasoning_effort: ReasoningEffort,
    pub kvcache_enabled: bool,
    pub kvcache_debug_enabled: bool,
    pub compact: config::CompactSection,
    pub hooker: HookerRegistryConfig,
    pub operation_backend: Option<crate::gateway::backend::GatewayBackendConfig>,
    pub skills_config: skill::SkillsConfig,
    pub subagent: std::collections::BTreeMap<String, config::SubagentRoleConfig>,
}

// ---------------------------------------------------------------------------
// Helpers for CLI gateway integration
// ---------------------------------------------------------------------------

pub fn build_llm_provider(
    config: &CliConfig,
    agent_id: Option<String>,
) -> Result<Arc<LlmProviderWrapper>, Box<dyn std::error::Error>> {
    let llm_config: LlmProviderConfig = {
        let mut c = LlmProviderConfig::new(&config.provider, &config.model);
        if let Some(ref key) = config.api_key {
            c = c.with_api_key(key);
        }
        if let Some(ref base) = config.api_base {
            c = c.with_api_base(base);
        }
        c
    };
    Ok(Arc::new(create_llm_provider(&llm_config, agent_id, None)?))
}

pub fn build_compression_pipeline(
    config: &CliConfig,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> Result<Arc<dyn CompressionPipeline>, Box<dyn std::error::Error>> {
    let estimator = Arc::new(
        RoughTokenEstimator::try_new(RoughTokenEstimatorConfig {
            chars_per_token: 4,
            message_overhead_tokens: 4,
            tool_use_overhead_tokens: 8,
            tool_result_overhead_tokens: 8,
            image_block_overhead_tokens: 256,
            document_block_overhead_tokens: 256,
        })
        .map_err(|e| format!("token estimator: {e}"))?,
    );
    let cc = &config.compact;
    let context_manager_config = ContextManagerConfig {
        thresholds: ContextThresholds {
            warning_ratio: cc.warning_ratio.unwrap_or(0.6),
            auto_compact_ratio: cc.auto_compact_ratio.unwrap_or(0.75),
            blocking_ratio: cc.blocking_ratio.unwrap_or(0.9),
        },
        micro_policy: MicroCompactionPolicy {
            stale_tool_pair_after_ms: 120_000,
            preserve_recent_messages: 6,
        },
        summary_budget: SummaryCompressionBudget {
            max_summary_tokens: cc.summary_max_tokens.unwrap_or(1024),
            preserve_tail_messages: cc.summary_preserve_tail.unwrap_or(4),
        },
        snip_preserve_tail_messages: cc.snip_preserve_tail.unwrap_or(6),
        collapse_preserve_tail_messages: cc.collapse_preserve_tail.unwrap_or(4),
        session_memory_compaction: None,
        snip_stale_after_ms: cc.snip_stale_after_ms.unwrap_or(3_600_000),
    };
    let compression_pipeline: Arc<dyn CompressionPipeline> = Arc::new(
        ContextManager::new(
            estimator,
            context_manager_config,
            Arc::clone(llm_provider),
            CompletionConfig {
                max_tokens: cc.summary_llm_max_tokens.unwrap_or(4096),
                temperature: 0.2,
            },
        )
        .map_err(|e| format!("context manager: {e}"))?,
    );
    Ok(compression_pipeline)
}

pub async fn resolve_effective_context_window(
    config: &CliConfig,
    llm_provider: &Arc<LlmProviderWrapper>,
) -> usize {
    if let Some(configured) = config.context_window.filter(|value| *value > 0) {
        return configured;
    }

    let resolved = resolve_config(ResolveInput {
        provider: Some(config.provider.clone()),
        protocol: None,
        api_key: config.api_key.clone(),
        api_key_env: config.api_key_env.clone(),
        base_url: config.api_base.clone(),
    });

    match resolved {
        Ok(resolved) => match resolve_model_context_length(&resolved, &config.model).await {
            Ok(Some(context_window)) => match usize::try_from(context_window) {
                Ok(value) if value > 0 => return value,
                Ok(_) => {}
                Err(_) => {
                    tracing::warn!(
                        provider = %config.provider,
                        model = %config.model,
                        context_window,
                        "dynamic CLI context window does not fit usize; falling back"
                    );
                }
            },
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    provider = %config.provider,
                    model = %config.model,
                    error = %error,
                    "failed to dynamically resolve CLI context window; falling back"
                );
            }
        },
        Err(error) => {
            tracing::warn!(
                provider = %config.provider,
                model = %config.model,
                error = %error,
                "failed to resolve CLI provider config for dynamic context window lookup; falling back"
            );
        }
    }

    llm_provider.capabilities().max_context_window.max(1)
}

#[cfg(test)]
mod tests {
    use super::{resolve_effective_context_window, CliConfig};
    use agent_contracts::{LlmProvider, ProviderCapabilities};
    use agent_types::{LlmError, LlmRequest, LlmResponse, StreamChunk};
    use async_trait::async_trait;
    use serde_json::Value;
    use std::sync::Arc;

    struct DummyProvider {
        capabilities: ProviderCapabilities,
    }

    #[async_trait]
    impl LlmProvider for DummyProvider {
        async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
            unimplemented!("not needed for cli context window tests")
        }

        async fn complete_stream(
            &self,
            _request: &LlmRequest,
            _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
        ) -> Result<LlmResponse, LlmError> {
            unimplemented!("not needed for cli context window tests")
        }

        fn capabilities(&self) -> &ProviderCapabilities {
            &self.capabilities
        }
    }

    fn test_config() -> CliConfig {
        CliConfig {
            skills_config: skill::SkillsConfig::default(),
            kvcache_debug_enabled: false,
            provider: "openai".to_string(),
            model: "gpt-4.1".to_string(),
            api_key: None,
            api_key_env: None,
            api_base: None,
            trace: Value::Object(serde_json::Map::new()),
            system_prompt: "test".to_string(),
            max_turns: 1,
            enable_tools: false,
            context_window: None,
            reasoning_effort: Default::default(),
            kvcache_enabled: true,
            compact: crate::cli::config::CompactSection::default(),
            hooker: Default::default(),
            operation_backend: None,
            subagent: Default::default(),
        }
    }

    fn test_provider(max_context_window: usize) -> Arc<super::LlmProviderWrapper> {
        Arc::new(super::LlmProviderWrapper::new(
            Arc::new(DummyProvider {
                capabilities: ProviderCapabilities {
                    supports_streaming: true,
                    supports_tool_calls: false,
                    supports_json_mode: false,
                    max_context_window,
                    model_name: "dummy".to_string(),
                },
            }),
            None,
            None,
        ))
    }

    #[tokio::test]
    async fn cli_context_window_prefers_explicit_config() {
        let mut config = test_config();
        config.context_window = Some(54321);

        let resolved = resolve_effective_context_window(&config, &test_provider(12345)).await;
        assert_eq!(resolved, 54321);
    }

    #[tokio::test]
    async fn cli_context_window_falls_back_to_provider_capability() {
        let mut config = test_config();
        config.provider = "unknown-provider".to_string();

        let resolved = resolve_effective_context_window(&config, &test_provider(12345)).await;
        assert_eq!(resolved, 12345);
    }
}
