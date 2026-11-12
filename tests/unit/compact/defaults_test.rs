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

struct SummaryProvider {
    capabilities: ProviderCapabilities,
    response: String,
}

#[async_trait]
impl LlmProvider for SummaryProvider {
    async fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        assert_eq!(
            matches!(
                request.response_format,
                agent_types::ResponseFormat::JsonObject
            ),
            self.capabilities.supports_json_mode,
            "summary request must honor the provider's JSON capability"
        );
        Ok(LlmResponse {
            message: agent_types::AssistantMessage {
                text: Some(self.response.clone()),
                reasoning_content: None,
                tool_calls: vec![],
                usage: Default::default(),
                stop_reason: agent_types::StopReason::EndTurn,
            },
            kv_cache_chunk_hashes: vec![],
        })
    }

    async fn complete_stream(
        &self,
        _: &LlmRequest,
        _: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        panic!("summary must use non-streaming completion")
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}

async fn compact_with_capability(
    json: bool,
    response: &str,
) -> Result<agent_types::compression::CompressedView, agent_contracts::CompressionError> {
    let provider = SummaryProvider {
        capabilities: ProviderCapabilities {
            supports_streaming: false,
            supports_tool_calls: false,
            supports_json_mode: json,
            max_context_window: 4096,
            model_name: "summary-model".into(),
        },
        response: response.into(),
    };
    let pipeline = build_context_manager(
        None,
        Arc::new(LlmProviderWrapper::new(Arc::new(provider), None, None)),
    )
    .unwrap();
    let mut messages = vec![ChatMessage::user("original task")];
    for i in 0..10 {
        let mut message = ChatMessage::text(MessageRole::Assistant, "detail ".repeat(1000), i);
        message.message_id = Some(format!("m{i}"));
        messages.push(message);
    }
    let policy = CompactionPolicy::from_budget(&TokenBudgetConfig {
        total_budget: 4096,
        reserved_for_output: 512,
        reserved_for_system: 256,
        hard_limit_ratio: 1.0,
    });
    assert!(pipeline.analyze(&messages, &policy).should_compact);
    pipeline
        .compress(&messages, &policy, &Default::default())
        .await
}

#[tokio::test]
async fn summary_without_json_mode_accepts_plain_and_fenced_json() {
    for text in [
        r#"{"previous_context":[],"new_context":["Keep concrete decisions."]}"#,
        "```json\n{\"previous_context\":[],\"new_context\":[\"Keep concrete decisions.\"]}\n```",
    ] {
        let view = compact_with_capability(false, text).await.unwrap();
        assert!(view.removed_count > 0);
        assert!(view.summary.unwrap().contains("Keep concrete decisions."));
        assert_eq!(
            view.messages[0].blocks,
            ChatMessage::user("original task").blocks
        );
    }
}

#[tokio::test]
async fn summary_with_json_mode_keeps_existing_format() {
    let view = compact_with_capability(
        true,
        r#"{"previous_context":[],"new_context":["Keep concrete decisions."]}"#,
    )
    .await
    .unwrap();
    assert!(view.summary.unwrap().starts_with("Conversation summary:\n"));
    assert!(view.messages.iter().any(|message| message.blocks.iter().any(|block| matches!(block, ContentBlock::Text { text } if text.starts_with("auto_compact:\n")))));
}

#[tokio::test]
async fn invalid_summary_is_rejected_in_both_modes() {
    for json in [false, true] {
        let error = compact_with_capability(json, "not JSON").await.unwrap_err();
        assert!(error.to_string().contains("summary parse failed"));
    }
}

#[test]
fn rough_estimate_counts_only_reasoning_sent_with_assistant_tool_calls() {
    use agent_contracts::TokenEstimator;
    let estimator = RoughTokenEstimator::try_new(RoughTokenEstimatorConfig {
        chars_per_token: 4,
        message_overhead_tokens: 4,
        tool_use_overhead_tokens: 8,
        tool_result_overhead_tokens: 8,
        image_block_overhead_tokens: 256,
        document_block_overhead_tokens: 256,
    })
    .unwrap();
    for role in [MessageRole::Assistant, MessageRole::User] {
        for tool in [false, true] {
            let mut message = ChatMessage::text(role.clone(), "visible", 0);
            if tool {
                message.blocks.push(ContentBlock::ToolUse {
                    call_id: "c".into(),
                    tool_name: "lookup".into(),
                    input: serde_json::json!({}),
                });
            }
            let before = estimator.estimate_message_tokens(&message);
            message.reasoning_content = Some("r".repeat(400));
            let expected = if role == MessageRole::Assistant && tool {
                100
            } else {
                0
            };
            assert_eq!(
                estimator.estimate_message_tokens(&message),
                before + expected
            );
        }
    }
}
