use std::sync::Arc;

use agent_contracts::{CompressionError, CompressionPipeline, TokenBudgetPolicy, TokenEstimator};
use agent_llm::{ChatMessageExt, CompletionConfigExt};
use agent_types::compression::{CompressedView, CompressionMeta, ContextAnalysis};
use agent_types::{ChatMessage, CompletionConfig, ContentBlock, MessageRole, TokenBudgetConfig};
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;

use crate::{
    compaction::{
        apply_candidate_replacement, candidate_indexes, discover_boundary, CompactionBoundary,
        CompactionResult,
    },
    envelope::ContextEnvelope,
    policy::{CompactionPolicy, CompactionPolicyService, ContextThresholds},
    summary::{summarize_messages_with_previous, SummaryCompressionBudget},
    CompactError, CompactResult,
};

#[derive(Clone, Debug)]
pub struct ContextManagerConfig {
    pub thresholds: ContextThresholds,
    pub summary_budget: SummaryCompressionBudget,
    pub collapse_preserve_tail_messages: usize,
}

impl ContextManagerConfig {
    pub fn validate(&self) -> CompactResult<()> {
        self.thresholds.validate()?;
        self.summary_budget.validate()?;

        if self.collapse_preserve_tail_messages == 0 {
            return Err(CompactError::InvalidConfiguration {
                message: "tail preservation counts must be greater than zero".to_string(),
            });
        }

        Ok(())
    }
}

pub struct ContextManager {
    estimator: Arc<dyn TokenEstimator>,
    service: CompactionPolicyService,
    config: ContextManagerConfig,
    summary_provider: Arc<LlmProviderWrapper>,
    summary_request: CompletionConfig,
}

impl ContextManager {
    pub fn new(
        estimator: Arc<dyn TokenEstimator>,
        config: ContextManagerConfig,
        summary_provider: Arc<LlmProviderWrapper>,
        summary_request: CompletionConfig,
    ) -> CompactResult<Self> {
        config.validate()?;
        summary_request
            .validate()
            .map_err(|error| CompactError::InvalidConfiguration {
                message: error.to_string(),
            })?;
        let service = CompactionPolicyService::new(config.thresholds.clone())?;

        Ok(Self {
            estimator,
            service,
            config,
            summary_provider,
            summary_request,
        })
    }

    pub fn checked_analyze(
        &self,
        messages: &[ChatMessage],
        budget: &TokenBudgetConfig,
    ) -> CompactResult<ContextAnalysis> {
        let estimated_tokens = self.estimator.estimate_messages_tokens(messages);
        let policy = CompactionPolicy::from_budget(budget);
        self.service.analyze(estimated_tokens, &policy)
    }

    pub fn build_envelope(
        &self,
        messages: &[ChatMessage],
        budget: &TokenBudgetConfig,
        summary: Option<&str>,
    ) -> CompactResult<ContextEnvelope> {
        let analysis = self.checked_analyze(messages, budget)?;
        Ok(ContextEnvelope::build(
            messages,
            summary,
            self.estimator.as_ref(),
            analysis,
            self.config.summary_budget.preserve_tail_messages,
        ))
    }

    fn resolve_candidates(
        &self,
        messages: &[ChatMessage],
        preserve_tail_messages: usize,
        boundary: Option<CompactionBoundary>,
    ) -> CompactResult<(Option<CompactionBoundary>, Vec<usize>)> {
        let effective_boundary =
            boundary.or_else(|| discover_boundary(messages, preserve_tail_messages));
        let candidates = candidate_indexes(
            messages,
            preserve_tail_messages,
            effective_boundary.as_ref(),
        )?;
        Ok((effective_boundary, candidates))
    }

    async fn context_collapse(
        &self,
        messages: &[ChatMessage],
        meta: &CompressionMeta,
        boundary: Option<CompactionBoundary>,
        preserve_tail_messages: usize,
    ) -> CompactResult<CompactionResult> {
        let (boundary, candidates) =
            self.resolve_candidates(messages, preserve_tail_messages, boundary)?;

        if candidates.is_empty() {
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                summary: None,
                boundary,
                updated_meta: meta.clone(),
                estimated_tokens: self.estimator.estimate_messages_tokens(messages),
            });
        }

        let candidate_messages = candidates
            .iter()
            .map(|index| messages[*index].clone())
            .collect::<Vec<_>>();

        let summary = summarize_messages_with_previous(
            &candidate_messages,
            meta.compact_summary.as_deref(),
            self.estimator.as_ref(),
            &self.config.summary_budget,
            self.summary_provider.as_ref(),
            &self.summary_request,
        )
        .await?;
        let inherited_message_id = candidate_messages
            .iter()
            .find_map(|message| message.message_id.clone());
        let collapse_message = ChatMessage::new(
            MessageRole::Assistant,
            vec![ContentBlock::Text {
                text: format!("context_collapse:\n{}", summary.summary),
            }],
            inherited_message_id,
            candidate_messages
                .last()
                .map(|message| message.timestamp_ms)
                .unwrap_or_default(),
            None,
        );
        let collapsed_messages =
            apply_candidate_replacement(messages, &candidates, Some(collapse_message));
        let mut updated_meta = meta.clone();
        updated_meta.last_collapse_index = candidates.iter().copied().max();
        updated_meta.compact_summary = Some(summary.summary.clone());

        Ok(CompactionResult {
            estimated_tokens: self.estimator.estimate_messages_tokens(&collapsed_messages),
            messages: collapsed_messages,
            summary: Some(summary.summary),
            boundary,
            updated_meta,
        })
    }

    async fn auto_compact(
        &self,
        messages: &[ChatMessage],
        meta: &CompressionMeta,
        boundary: Option<CompactionBoundary>,
        preserve_tail_messages: usize,
    ) -> CompactResult<CompactionResult> {
        let (boundary, candidates) =
            self.resolve_candidates(messages, preserve_tail_messages, boundary)?;

        if candidates.is_empty() {
            // All messages are protected (first user + tail) — nothing to compact.
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                summary: meta.compact_summary.clone(),
                boundary,
                updated_meta: meta.clone(),
                estimated_tokens: self.estimator.estimate_messages_tokens(messages),
            });
        }

        let candidate_messages = candidates
            .iter()
            .map(|index| messages[*index].clone())
            .collect::<Vec<_>>();

        let candidates_include_summary = candidate_messages.iter().any(is_compression_summary);
        let previous_summary = if candidates_include_summary {
            None
        } else {
            meta.compact_summary.as_deref()
        };

        let summary = summarize_messages_with_previous(
            &candidate_messages,
            previous_summary,
            self.estimator.as_ref(),
            &self.config.summary_budget,
            self.summary_provider.as_ref(),
            &self.summary_request,
        )
        .await?;
        let inherited_message_id = candidate_messages
            .iter()
            .find_map(|message| message.message_id.clone());
        let summary_message = ChatMessage::new(
            MessageRole::Assistant,
            vec![ContentBlock::Text {
                text: format!("auto_compact:\n{}", summary.summary),
            }],
            inherited_message_id,
            candidate_messages
                .last()
                .map(|message| message.timestamp_ms)
                .unwrap_or_default(),
            None,
        );
        let compacted_messages =
            apply_candidate_replacement(messages, &candidates, Some(summary_message));
        let mut updated_meta = meta.clone();
        updated_meta.compact_summary = Some(summary.summary.clone());
        updated_meta.last_compact_turn = Some(meta.last_compact_turn.unwrap_or_default() + 1);

        Ok(CompactionResult {
            estimated_tokens: self.estimator.estimate_messages_tokens(&compacted_messages),
            messages: compacted_messages,
            summary: Some(summary.summary),
            boundary,
            updated_meta,
        })
    }
}

#[async_trait]
impl CompressionPipeline for ContextManager {
    fn analyze(&self, messages: &[ChatMessage], budget: &dyn TokenBudgetPolicy) -> ContextAnalysis {
        let config = budget_to_config(budget);
        self.checked_analyze(messages, &config)
            .expect("ContextManager.analyze received an invalid token budget")
    }

    async fn compress(
        &self,
        messages: &[ChatMessage],
        budget: &dyn TokenBudgetPolicy,
        meta: &CompressionMeta,
    ) -> Result<CompressedView, CompressionError> {
        let budget = budget_to_config(budget);
        let mut analysis = self
            .checked_analyze(messages, &budget)
            .map_err(CompressionError::from)?;
        // History must only ever be rewritten with a summary standing in for
        // the removed messages, so below the auto-compact threshold the
        // pipeline stays a no-op instead of trimming context silently.
        if !analysis.should_compact {
            return Ok(CompressedView {
                messages: messages.to_vec(),
                removed_count: 0,
                summary: meta.compact_summary.clone(),
                updated_meta: meta.clone(),
                estimated_tokens: analysis.estimated_tokens,
            });
        }

        let policy = CompactionPolicy::from_budget(&budget);
        let history_limit = policy.history_limit().map_err(CompressionError::from)?;
        let mut working_messages = messages.to_vec();
        let mut updated_meta = meta.clone();
        let mut final_summary = meta.compact_summary.clone();

        // protect_first_user=true: in the pipeline flow, the first user message
        // (original task instructions) must never be compacted away.
        let result = self
            .context_collapse(
                &working_messages,
                &updated_meta,
                None,
                self.config.collapse_preserve_tail_messages,
            )
            .await
            .map_err(CompressionError::from)?;
        if result.messages != working_messages {
            working_messages = result.messages;
            if result.summary.is_some() {
                final_summary = result.summary.clone();
            }
            updated_meta = result.updated_meta;
            analysis = self
                .checked_analyze(&working_messages, &budget)
                .map_err(CompressionError::from)?;
        }

        if analysis.estimated_tokens > history_limit {
            let result = self
                .auto_compact(
                    &working_messages,
                    &updated_meta,
                    None,
                    self.config.summary_budget.preserve_tail_messages,
                )
                .await
                .map_err(CompressionError::from)?;
            working_messages = result.messages;
            final_summary = result.summary;
            updated_meta = result.updated_meta;
        }

        let removed_count = messages.len().saturating_sub(working_messages.len());

        Ok(CompressedView {
            estimated_tokens: self.estimator.estimate_messages_tokens(&working_messages),
            messages: working_messages,
            removed_count,
            summary: final_summary,
            updated_meta,
        })
    }
}

/// A compression summary message is one produced by context_collapse or auto_compact.
fn is_compression_summary(message: &ChatMessage) -> bool {
    if !matches!(message.role, MessageRole::Assistant) {
        return false;
    }
    message.blocks.iter().any(|block| match block {
        ContentBlock::Text { text } => {
            text.starts_with("context_collapse:\n") || text.starts_with("auto_compact:\n")
        }
        _ => false,
    })
}

fn budget_to_config(budget: &dyn TokenBudgetPolicy) -> TokenBudgetConfig {
    TokenBudgetConfig {
        total_budget: budget.total_budget(),
        reserved_for_output: budget.reserved_for_output(),
        reserved_for_system: budget.reserved_for_system(),
        hard_limit_ratio: budget.hard_limit_ratio(),
    }
}
