use super::*;
use crate::CompactionPolicy;
use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_llm::ChatMessageExt;
use agent_types::{
    compression::ContextSeverity, ChatMessage, ContentBlock, LlmError, LlmRequest, LlmResponse,
    MessageRole, StreamChunk, TokenBudgetConfig,
};
use async_trait::async_trait;
use std::sync::Arc;

/// Minimal no-op provider: `analyze` never calls the LLM, so this is safe
/// to use when only exercising the analyzer side of the pipeline.
struct NoopLlmProvider;

#[async_trait]
impl LlmProvider for NoopLlmProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        Err(LlmError::ProviderNotFound("NoopLlmProvider".to_string()))
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        Err(LlmError::ProviderNotFound("NoopLlmProvider".to_string()))
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        // `analyze` does not read capabilities; a static placeholder is
        // enough for tests that never invoke the LLM.
        static CAPS: std::sync::OnceLock<ProviderCapabilities> = std::sync::OnceLock::new();
        CAPS.get_or_init(|| ProviderCapabilities {
            supports_streaming: false,
            supports_tool_calls: false,
            supports_json_mode: false,
            max_context_window: 0,
            model_name: "noop".to_string(),
        })
    }
}

fn wrapper() -> Arc<LlmProviderWrapper> {
    Arc::new(LlmProviderWrapper::new(
        Arc::new(NoopLlmProvider) as Arc<dyn LlmProvider>,
        None,
        None,
    ))
}

fn large_messages() -> Vec<ChatMessage> {
    // ~200k chars -> well above any history_limit derived from a small
    // total_budget, so a real ContextManager MUST report Blocking.
    let big = "x".repeat(200_000);
    vec![ChatMessage::new(
        MessageRole::User,
        vec![ContentBlock::Text { text: big }],
        None,
        0,
        None,
    )]
}

/// Regression guard: `build_context_manager(None, _)` MUST yield a real
/// `ContextManager`, NOT a `PassthroughCompressionPipeline`. The passthrough
/// analyzer hard-codes `severity = Normal` / `should_compact = false`, so
/// observing `Blocking` here proves the no-op fallback is gone.
#[tokio::test]
async fn none_overrides_builds_real_context_manager() {
    let pipeline = build_context_manager(None, wrapper()).expect("must build with defaults");

    let budget = TokenBudgetConfig {
        total_budget: 4096,
        reserved_for_output: 512,
        reserved_for_system: 256,
        hard_limit_ratio: 1.0,
    };
    let policy = CompactionPolicy::from_budget(&budget);

    let messages = large_messages();
    let analysis = pipeline.analyze(&messages, &policy);

    assert!(
        analysis.should_compact,
        "real ContextManager must flag should_compact on oversized input"
    );
    assert_eq!(
        analysis.severity,
        ContextSeverity::Blocking,
        "severity must be Blocking for oversized input, got {:?}",
        analysis.severity
    );
}

/// Overrides take effect: a tiny `blocking_ratio` keeps the same pipeline
/// shape but lowers the threshold so even a modest message trips Blocking.
#[tokio::test]
async fn overrides_are_applied() {
    let overrides = CompactOverrides {
        warning_ratio: Some(0.1),
        auto_compact_ratio: Some(0.2),
        blocking_ratio: Some(0.3),
        ..Default::default()
    };
    let pipeline =
        build_context_manager(Some(&overrides), wrapper()).expect("must build with overrides");

    let budget = TokenBudgetConfig {
        total_budget: 4096,
        reserved_for_output: 512,
        reserved_for_system: 256,
        hard_limit_ratio: 1.0,
    };
    let policy = CompactionPolicy::from_budget(&budget);

    let messages = large_messages();
    let analysis = pipeline.analyze(&messages, &policy);
    assert!(
        analysis.should_compact,
        "lowered thresholds must still flag oversized input"
    );
}
