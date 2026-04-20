pub mod config;

use std::sync::Arc;

use agent_contracts::CompressionPipeline;
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use agent_types::hook::HookerRegistryConfig;
use agent_types::CompletionConfig;
use compact::{
    ContextManager, ContextManagerConfig, ContextThresholds, MicroCompactionPolicy,
    RoughTokenEstimator, RoughTokenEstimatorConfig, SummaryCompressionBudget,
};
use llm_client::{create_llm_provider, LlmProviderConfig, LlmProviderWrapper};
use serde_json::Value;

// ---------------------------------------------------------------------------
// CliEventSink
// ---------------------------------------------------------------------------

pub struct CliEventSink {
    pub debug: bool,
}

impl agent_contracts::events::LoopEventSink for CliEventSink {
    fn on_turn_start(&self, agent_id: &agent_types::common::ids::AgentId, turn: u32) {
        if self.debug {
            eprintln!("--- turn {} [{}] ---", turn, agent_id.0);
        }
    }

    fn on_assistant_message(&self, agent_id: &agent_types::common::ids::AgentId, text: &str) {
        if self.debug {
            eprintln!("[assistant: {}] {}", agent_id.0, text);
        }
    }

    fn on_tool_result(
        &self,
        agent_id: &agent_types::common::ids::AgentId,
        event: &ToolResultEvent,
    ) {
        if self.debug {
            let status = if event.is_error { "ERR" } else { "OK" };
            eprintln!(
                "  [tool: {}] {} ({}) => {}",
                agent_id.0, event.tool_name, status, event.output_preview
            );
        }
    }

    fn on_loop_end(&self, agent_id: &agent_types::common::ids::AgentId, summary: &LoopEndSummary) {
        if self.debug {
            eprintln!(
                "--- end [{}] (turns={}, tokens={}, reason={}) ---",
                agent_id.0, summary.turn_count, summary.total_tokens, summary.stop_reason
            );
        }
    }
}

// ---------------------------------------------------------------------------
// CLI config (merged from file + CLI args)
// ---------------------------------------------------------------------------

pub struct CliConfig {
    pub provider: String,
    pub model: String,
    pub api_key: Option<String>,
    pub api_base: Option<String>,
    pub trace: Value,
    pub system_prompt: String,
    pub max_turns: u32,
    pub enable_tools: bool,
    pub context_window: usize,
    pub compact: config::CompactSection,
    pub hooker: HookerRegistryConfig,
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
