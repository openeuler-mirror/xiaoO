use agent_contracts::{CompressionError, CompressionPipeline, TokenBudgetPolicy};
use agent_types::compression::{
    CompressedView, CompressionMeta, ContextAnalysis, ContextSeverity, MicroCompactResult,
};
use agent_types::{ChatMessage, ContentBlock};
use async_trait::async_trait;

#[derive(Debug, Default, Clone, Copy)]
pub struct PassthroughCompressionPipeline;

impl PassthroughCompressionPipeline {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl CompressionPipeline for PassthroughCompressionPipeline {
    fn analyze(&self, messages: &[ChatMessage], budget: &dyn TokenBudgetPolicy) -> ContextAnalysis {
        let estimated_tokens = messages
            .iter()
            .flat_map(|message| message.blocks.iter())
            .map(|block| match block {
                ContentBlock::Text { text } => text.len(),
                ContentBlock::ToolUse {
                    call_id,
                    tool_name,
                    input,
                } => {
                    call_id.len()
                        + tool_name.len()
                        + serde_json::to_string(input).unwrap_or_default().len()
                }
                ContentBlock::ToolResult {
                    call_id,
                    tool_name,
                    output,
                    ..
                } => call_id.len() + tool_name.len() + output.len(),
                ContentBlock::Image { description } | ContentBlock::Document { description } => {
                    description.len()
                }
            })
            .sum::<usize>();
        let available_tokens = budget.available_budget().unwrap_or(0);

        ContextAnalysis {
            severity: ContextSeverity::Normal,
            estimated_tokens,
            should_compact: false,
            total_tokens: estimated_tokens,
            available_tokens,
            usage_ratio: if available_tokens == 0 {
                0.0
            } else {
                estimated_tokens as f64 / available_tokens as f64
            },
        }
    }

    async fn compress(
        &self,
        messages: &[ChatMessage],
        _budget: &dyn TokenBudgetPolicy,
        meta: &CompressionMeta,
    ) -> Result<CompressedView, CompressionError> {
        Ok(CompressedView {
            messages: messages.to_vec(),
            removed_count: 0,
            summary: None,
            updated_meta: meta.clone(),
            estimated_tokens: messages.len(),
        })
    }

    fn microcompact(&self, messages: &[ChatMessage], _now_ms: u64) -> MicroCompactResult {
        MicroCompactResult {
            applied: false,
            removed_count: 0,
            removed_call_ids: Vec::new(),
            messages: messages.to_vec(),
            token_delta: 0,
        }
    }
}
