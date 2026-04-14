use std::sync::Arc;

use agent_contracts::{CompressionError, CompressionPipeline, TokenBudgetPolicy, TokenEstimator};
use agent_llm::{ChatMessageExt, CompletionConfigExt};
use agent_types::compression::{
    CompressedView, CompressionMeta, ContextAnalysis, ContextSeverity, MicroCompactResult,
};
use agent_types::{ChatMessage, CompletionConfig, ContentBlock, MessageRole, TokenBudgetConfig};
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;
use memory::MemorySnapshot;

use crate::{
    compaction::{
        adjust_keep_start_for_invariants, apply_candidate_replacement, candidate_indexes,
        discover_boundary, CompactMode, CompactRequest, CompactionBoundary, CompactionDecision,
        CompactionResult,
    },
    envelope::ContextEnvelope,
    microcompact::{apply_microcompact, MicroCompactionPolicy},
    policy::{CompactionPolicy, CompactionPolicyService, ContextThresholds},
    summary::{summarize_messages_with_previous, SummaryCompressionBudget},
    CompactError, CompactResult,
};

#[derive(Clone, Debug)]
pub struct ContextManagerConfig {
    pub thresholds: ContextThresholds,
    pub micro_policy: MicroCompactionPolicy,
    pub summary_budget: SummaryCompressionBudget,
    pub snip_preserve_tail_messages: usize,
    pub collapse_preserve_tail_messages: usize,
    pub session_memory_compaction: Option<SessionMemoryCompactionPolicy>,
    /// Only snip messages older than this threshold (milliseconds).
    /// Messages within an active task are recent and should be summarized, not deleted.
    /// Default: 3_600_000 (1 hour).
    pub snip_stale_after_ms: u64,
}

#[derive(Clone, Debug)]
pub struct SessionMemoryCompactionPolicy {
    pub min_preserved_tokens: usize,
    pub min_preserved_text_messages: usize,
    pub max_preserved_tokens: usize,
    pub compact_max_section_tokens: usize,
    pub compact_max_total_tokens: usize,
}

impl SessionMemoryCompactionPolicy {
    pub fn validate(&self) -> CompactResult<()> {
        if self.min_preserved_tokens == 0
            || self.min_preserved_text_messages == 0
            || self.max_preserved_tokens == 0
            || self.compact_max_section_tokens == 0
            || self.compact_max_total_tokens == 0
        {
            return Err(CompactError::InvalidConfiguration {
                message: "session memory compaction policy values must be greater than zero"
                    .to_string(),
            });
        }

        if self.max_preserved_tokens < self.min_preserved_tokens {
            return Err(CompactError::InvalidConfiguration {
                message:
                    "session memory compaction max_preserved_tokens must be greater than or equal to min_preserved_tokens"
                        .to_string(),
            });
        }

        if self.compact_max_total_tokens < self.compact_max_section_tokens {
            return Err(CompactError::InvalidConfiguration {
                message:
                    "session memory compaction compact_max_total_tokens must be greater than or equal to compact_max_section_tokens"
                        .to_string(),
            });
        }

        Ok(())
    }
}

impl ContextManagerConfig {
    pub fn validate(&self) -> CompactResult<()> {
        self.thresholds.validate()?;
        self.summary_budget.validate()?;

        if self.snip_preserve_tail_messages == 0 || self.collapse_preserve_tail_messages == 0 {
            return Err(CompactError::InvalidConfiguration {
                message: "tail preservation counts must be greater than zero".to_string(),
            });
        }

        if let Some(policy) = &self.session_memory_compaction {
            policy.validate()?;
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

    pub fn decide_compaction(
        &self,
        messages: &[ChatMessage],
        analysis: &ContextAnalysis,
    ) -> CompactResult<Option<CompactionDecision>> {
        if !analysis.should_compact {
            return Ok(None);
        }

        let (mode, reason, preserve_tail_messages) = match analysis.severity {
            ContextSeverity::Normal => return Ok(None),
            ContextSeverity::Warning => (
                CompactMode::HistorySnip,
                "warning threshold exceeded".to_string(),
                self.config.snip_preserve_tail_messages,
            ),
            ContextSeverity::AutoCompact => (
                CompactMode::ContextCollapse,
                "auto compact threshold exceeded".to_string(),
                self.config.collapse_preserve_tail_messages,
            ),
            ContextSeverity::Blocking => (
                CompactMode::AutoCompact,
                "blocking threshold exceeded".to_string(),
                self.config.summary_budget.preserve_tail_messages,
            ),
        };

        Ok(Some(CompactionDecision {
            mode,
            boundary: discover_boundary(messages, preserve_tail_messages),
            reason,
        }))
    }

    pub async fn run_compaction_request(
        &self,
        request: CompactRequest,
        meta: &CompressionMeta,
    ) -> CompactResult<CompactionResult> {
        match request.mode {
            CompactMode::HistorySnip => self.history_snip(
                &request.messages,
                meta,
                request.boundary,
                self.config.snip_preserve_tail_messages,
            ),
            CompactMode::ContextCollapse => {
                self.context_collapse(
                    &request.messages,
                    meta,
                    request.boundary,
                    self.config.collapse_preserve_tail_messages,
                )
                .await
            }
            CompactMode::AutoCompact => {
                self.auto_compact(
                    &request.messages,
                    meta,
                    request.boundary,
                    self.config.summary_budget.preserve_tail_messages,
                )
                .await
            }
        }
    }

    pub fn try_session_memory_compaction(
        &self,
        snapshot: &MemorySnapshot,
        meta: &CompressionMeta,
    ) -> CompactResult<Option<CompactionResult>> {
        let Some(policy) = &self.config.session_memory_compaction else {
            return Ok(None);
        };
        let Some(session_memory) = snapshot.session_memory.as_ref() else {
            return Ok(None);
        };
        if session_memory.session_id != snapshot.session_id {
            return Err(CompactError::InvalidConfiguration {
                message: "snapshot.session_memory.session_id must match snapshot.session_id"
                    .to_string(),
            });
        }

        let keep_start =
            self.calculate_session_memory_keep_start(&snapshot.messages, session_memory, policy);
        if keep_start == 0 {
            return Ok(None);
        }

        let compacted_prefix = &snapshot.messages[..keep_start];
        let preserved_tail = snapshot.messages[keep_start..].to_vec();
        let truncated_summary = memory::truncate_session_memory_summary(
            &session_memory.summary,
            self.estimator.as_ref(),
            policy.compact_max_section_tokens,
            policy.compact_max_total_tokens,
        )
        .map_err(|error| match error {
            memory::MemoryError::InvalidConfiguration { message } => {
                CompactError::InvalidConfiguration { message }
            }
            memory::MemoryError::SessionMemoryBudgetExhausted { message } => {
                CompactError::SummaryBudgetExhausted { message }
            }
            other => CompactError::InvalidConfiguration {
                message: other.to_string(),
            },
        })?;
        let summary_message = ChatMessage::new(
            MessageRole::Assistant,
            vec![ContentBlock::Text {
                text: format!("session_memory_compact:\n{}", truncated_summary.summary),
            }],
            compacted_prefix
                .iter()
                .find_map(|message| message.message_id.clone()),
            compacted_prefix
                .last()
                .map(|message| message.timestamp_ms)
                .unwrap_or_default(),
            None,
        );
        let mut compacted_messages = vec![summary_message];
        compacted_messages.extend(preserved_tail);

        let mut updated_meta = meta.clone();
        updated_meta.compact_summary = Some(truncated_summary.summary.clone());
        updated_meta.last_compact_turn = Some(meta.last_compact_turn.unwrap_or_default() + 1);

        Ok(Some(CompactionResult {
            estimated_tokens: self.estimator.estimate_messages_tokens(&compacted_messages),
            messages: compacted_messages,
            summary: Some(truncated_summary.summary),
            boundary: None,
            updated_meta,
        }))
    }

    pub async fn compress_snapshot(
        &self,
        snapshot: &MemorySnapshot,
        budget: &TokenBudgetConfig,
        meta: &CompressionMeta,
    ) -> Result<CompressedView, CompressionError> {
        let history_limit = CompactionPolicy::from_budget(budget)
            .history_limit()
            .map_err(CompressionError::from)?;
        if let Some(result) = self
            .try_session_memory_compaction(snapshot, meta)
            .map_err(CompressionError::from)?
        {
            if result.estimated_tokens <= history_limit {
                return Ok(CompressedView {
                    estimated_tokens: result.estimated_tokens,
                    messages: result.messages,
                    removed_count: 0,
                    summary: result.summary,
                    updated_meta: result.updated_meta,
                });
            }
        }

        let policy = CompactionPolicy::from_budget(budget);
        self.compress(&snapshot.messages, &policy, meta).await
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

    fn history_snip(
        &self,
        messages: &[ChatMessage],
        meta: &CompressionMeta,
        boundary: Option<CompactionBoundary>,
        preserve_tail_messages: usize,
    ) -> CompactResult<CompactionResult> {
        let (boundary, candidates) =
            self.resolve_candidates(messages, preserve_tail_messages, boundary)?;

        // HistorySnip only deletes stale messages (older than snip_stale_after_ms).
        // Active-task messages should be summarized (ContextCollapse/AutoCompact), not deleted.
        // Compression summary messages are also protected — they can only be *absorbed* by
        // a summarization pass, never silently deleted.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let stale_threshold = self.config.snip_stale_after_ms;
        let stale_candidates: Vec<usize> = candidates
            .into_iter()
            .filter(|&idx| {
                let msg = &messages[idx];
                if is_compression_summary(msg) {
                    return false;
                }
                if stale_threshold == 0 {
                    return true; // no threshold configured, allow all
                }
                if msg.timestamp_ms == 0 {
                    return false; // unknown timestamp, conservatively keep
                }
                now_ms.saturating_sub(msg.timestamp_ms) >= stale_threshold
            })
            .collect();

        if stale_candidates.is_empty() {
            return Ok(CompactionResult {
                messages: messages.to_vec(),
                summary: None,
                boundary,
                updated_meta: meta.clone(),
                estimated_tokens: self.estimator.estimate_messages_tokens(messages),
            });
        }

        let trimmed_messages = apply_candidate_replacement(messages, &stale_candidates, None);
        let mut updated_meta = meta.clone();
        updated_meta.last_snip_index = stale_candidates.iter().copied().max();

        Ok(CompactionResult {
            estimated_tokens: self.estimator.estimate_messages_tokens(&trimmed_messages),
            messages: trimmed_messages,
            summary: None,
            boundary,
            updated_meta,
        })
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

        let candidates_include_summary =
            candidate_messages.iter().any(|m| is_compression_summary(m));
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

    fn calculate_session_memory_keep_start(
        &self,
        messages: &[ChatMessage],
        session_memory: &memory::SessionMemorySummary,
        policy: &SessionMemoryCompactionPolicy,
    ) -> usize {
        if messages.is_empty() {
            return 0;
        }

        let mut keep_start = session_memory
            .summarized_through_message_id
            .as_deref()
            .and_then(|message_id| {
                messages
                    .iter()
                    .rposition(|message| message.message_id.as_deref() == Some(message_id))
                    .map(|index| index + 1)
            })
            .or_else(|| {
                session_memory
                    .summarized_through_timestamp_ms
                    .map(|timestamp_ms| {
                        messages
                            .iter()
                            .position(|message| message.timestamp_ms > timestamp_ms)
                            .unwrap_or(messages.len())
                    })
            })
            .unwrap_or(messages.len());

        let mut preserved_tokens = self
            .estimator
            .estimate_messages_tokens(&messages[keep_start..]);
        let mut preserved_text_messages = count_text_messages(&messages[keep_start..]);

        if preserved_tokens >= policy.max_preserved_tokens {
            return adjust_keep_start_for_invariants(messages, keep_start, 0);
        }

        while keep_start > 0
            && (preserved_tokens < policy.min_preserved_tokens
                || preserved_text_messages < policy.min_preserved_text_messages)
        {
            let next_index = keep_start - 1;
            let next_message = &messages[next_index];
            let next_tokens = self.estimator.estimate_message_tokens(next_message);
            if preserved_tokens + next_tokens > policy.max_preserved_tokens {
                break;
            }

            keep_start = next_index;
            preserved_tokens += next_tokens;
            preserved_text_messages += usize::from(message_has_text_content(next_message));
        }

        adjust_keep_start_for_invariants(messages, keep_start, 0)
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
        let initial_decision = self
            .decide_compaction(messages, &analysis)
            .map_err(CompressionError::from)?;
        if initial_decision.is_none() {
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
        if analysis.should_compact && !matches!(analysis.severity, ContextSeverity::Normal) {
            let result = self
                .history_snip(
                    &working_messages,
                    &updated_meta,
                    None,
                    self.config.snip_preserve_tail_messages,
                )
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
        }

        if matches!(
            analysis.severity,
            ContextSeverity::AutoCompact | ContextSeverity::Blocking
        ) {
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

    fn microcompact(&self, messages: &[ChatMessage], now_ms: u64) -> MicroCompactResult {
        apply_microcompact(
            messages,
            now_ms,
            self.estimator.as_ref(),
            &self.config.micro_policy,
        )
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

fn count_text_messages(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .filter(|message| message_has_text_content(message))
        .count()
}

fn message_has_text_content(message: &ChatMessage) -> bool {
    message.blocks.iter().any(|block| {
        matches!(
            block,
            ContentBlock::Text { .. } | ContentBlock::Image { .. } | ContentBlock::Document { .. }
        )
    })
}
