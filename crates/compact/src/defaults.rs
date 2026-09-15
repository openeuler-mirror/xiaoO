//! Shared default-construction entry point for the compression pipeline.
//!
//! Both the daemon (`apps/serverside`) and the local CLI/TUI (`apps/endside`)
//! route through [`build_context_manager`] so that:
//!
//! 1. `[compact]` missing from the config file no longer silently falls back
//!    to `PassthroughCompressionPipeline` (a no-op that leaves the agent loop
//!    emitting `Pre-check failed` / `context compression triggered` with
//!    `removed=0` forever). Instead, a real `ContextManager` is built using
//!    the defaults below.
//! 2. Both modes share a single source of truth for default thresholds /
//!    estimator tuning, so they can never drift apart again.

use std::sync::Arc;

use agent_contracts::CompressionPipeline;
use agent_types::CompletionConfig;
use llm_client::LlmProviderWrapper;

use crate::{
    CompactResult, ContextManager, ContextManagerConfig, ContextThresholds, MicroCompactionPolicy,
    RoughTokenEstimator, RoughTokenEstimatorConfig, SummaryCompressionBudget,
};

/// Optional overrides for every field the user may set under `[compact]`.
///
/// Every field is `Option`; `Default::default()` yields all-`None`, which
/// [`build_context_manager`] interprets as "use the built-in defaults".
#[derive(Debug, Clone, Default)]
pub struct CompactOverrides {
    pub warning_ratio: Option<f64>,
    pub auto_compact_ratio: Option<f64>,
    pub blocking_ratio: Option<f64>,
    pub snip_stale_after_ms: Option<u64>,
    pub snip_preserve_tail: Option<usize>,
    pub collapse_preserve_tail: Option<usize>,
    pub summary_max_tokens: Option<usize>,
    pub summary_preserve_tail: Option<usize>,
    pub summary_llm_max_tokens: Option<usize>,
}

// --- Default constants (single source of truth) -----------------------------

pub const DEFAULT_WARNING_RATIO: f64 = 0.6;
pub const DEFAULT_AUTO_COMPACT_RATIO: f64 = 0.75;
pub const DEFAULT_BLOCKING_RATIO: f64 = 0.9;

pub const DEFAULT_SNIP_STALE_AFTER_MS: u64 = 3_600_000;
pub const DEFAULT_SNIP_PRESERVE_TAIL: usize = 6;
pub const DEFAULT_COLLAPSE_PRESERVE_TAIL: usize = 4;

pub const DEFAULT_SUMMARY_MAX_TOKENS: usize = 1024;
pub const DEFAULT_SUMMARY_PRESERVE_TAIL: usize = 4;
pub const DEFAULT_SUMMARY_LLM_MAX_TOKENS: usize = 4096;
pub const DEFAULT_SUMMARY_TEMPERATURE: f64 = 0.2;

pub const DEFAULT_STALE_TOOL_PAIR_AFTER_MS: u64 = 120_000;
pub const DEFAULT_PRESERVE_RECENT_MESSAGES: usize = 6;

// --- Estimator tuning (mirrors the original local-CLI values) --------------

const ESTIMATOR_CHARS_PER_TOKEN: usize = 4;
const ESTIMATOR_MESSAGE_OVERHEAD: usize = 4;
const ESTIMATOR_TOOL_USE_OVERHEAD: usize = 8;
const ESTIMATOR_TOOL_RESULT_OVERHEAD: usize = 8;
const ESTIMATOR_IMAGE_BLOCK_OVERHEAD: usize = 256;
const ESTIMATOR_DOCUMENT_BLOCK_OVERHEAD: usize = 256;

/// Build a real [`ContextManager`] compression pipeline from `overrides`
/// (user-supplied) layered on top of the defaults above.
///
/// Pass `overrides = None` to use every default — this is the path that
/// fixes the daemon's previous `None -> Passthrough` regression, where a
/// missing `[compact]` section silently disabled compression.
pub fn build_context_manager(
    overrides: Option<&CompactOverrides>,
    llm_provider: Arc<LlmProviderWrapper>,
) -> CompactResult<Arc<dyn CompressionPipeline>> {
    let ov = overrides.cloned().unwrap_or_default();

    let estimator = Arc::new(RoughTokenEstimator::try_new(RoughTokenEstimatorConfig {
        chars_per_token: ESTIMATOR_CHARS_PER_TOKEN,
        message_overhead_tokens: ESTIMATOR_MESSAGE_OVERHEAD,
        tool_use_overhead_tokens: ESTIMATOR_TOOL_USE_OVERHEAD,
        tool_result_overhead_tokens: ESTIMATOR_TOOL_RESULT_OVERHEAD,
        image_block_overhead_tokens: ESTIMATOR_IMAGE_BLOCK_OVERHEAD,
        document_block_overhead_tokens: ESTIMATOR_DOCUMENT_BLOCK_OVERHEAD,
    })?);

    let config = ContextManagerConfig {
        thresholds: ContextThresholds {
            warning_ratio: ov.warning_ratio.unwrap_or(DEFAULT_WARNING_RATIO),
            auto_compact_ratio: ov.auto_compact_ratio.unwrap_or(DEFAULT_AUTO_COMPACT_RATIO),
            blocking_ratio: ov.blocking_ratio.unwrap_or(DEFAULT_BLOCKING_RATIO),
        },
        micro_policy: MicroCompactionPolicy {
            stale_tool_pair_after_ms: DEFAULT_STALE_TOOL_PAIR_AFTER_MS,
            preserve_recent_messages: DEFAULT_PRESERVE_RECENT_MESSAGES,
        },
        summary_budget: SummaryCompressionBudget {
            max_summary_tokens: ov.summary_max_tokens.unwrap_or(DEFAULT_SUMMARY_MAX_TOKENS),
            preserve_tail_messages: ov
                .summary_preserve_tail
                .unwrap_or(DEFAULT_SUMMARY_PRESERVE_TAIL),
        },
        snip_preserve_tail_messages: ov.snip_preserve_tail.unwrap_or(DEFAULT_SNIP_PRESERVE_TAIL),
        collapse_preserve_tail_messages: ov
            .collapse_preserve_tail
            .unwrap_or(DEFAULT_COLLAPSE_PRESERVE_TAIL),
        session_memory_compaction: None,
        snip_stale_after_ms: ov
            .snip_stale_after_ms
            .unwrap_or(DEFAULT_SNIP_STALE_AFTER_MS),
    };

    let completion = CompletionConfig {
        max_tokens: ov
            .summary_llm_max_tokens
            .unwrap_or(DEFAULT_SUMMARY_LLM_MAX_TOKENS),
        temperature: DEFAULT_SUMMARY_TEMPERATURE,
    };

    let cm = ContextManager::new(estimator, config, llm_provider, completion)?;
    Ok(Arc::new(cm))
}

#[cfg(test)]
#[path = "../../../tests/unit/compact/defaults_test.rs"]
mod tests;
