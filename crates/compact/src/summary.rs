use std::collections::BTreeSet;

use agent_contracts::TokenEstimator;
use agent_llm::{ChatMessageExt, CompletionConfigExt, LlmRequestExt, MessageRoleExt};
use agent_types::{ChatMessage, CompletionConfig, ContentBlock, LlmRequest, ResponseFormat};
use llm_client::LlmProviderWrapper;
use serde::{Deserialize, Serialize};

use crate::{CompactError, CompactResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryCompressionBudget {
    pub max_summary_tokens: usize,
    pub preserve_tail_messages: usize,
}

impl SummaryCompressionBudget {
    pub fn validate(&self) -> CompactResult<()> {
        if self.max_summary_tokens == 0 {
            return Err(CompactError::InvalidConfiguration {
                message: "max_summary_tokens must be greater than zero".to_string(),
            });
        }

        if self.preserve_tail_messages == 0 {
            return Err(CompactError::InvalidConfiguration {
                message: "preserve_tail_messages must be greater than zero".to_string(),
            });
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryCompressionResult {
    pub summary: String,
    pub covered_message_count: usize,
    pub estimated_tokens: usize,
    pub compressed_line_count: usize,
    pub removed_duplicate_lines: usize,
    pub omitted_line_count: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SummaryLineKind {
    Header,
    Scope,
    PreviousSectionHeader,
    NewSectionHeader,
    PreviousDetail,
    NewDetail,
    OmissionNotice,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SummaryLine {
    text: String,
    kind: SummaryLineKind,
}

pub async fn summarize_messages(
    messages: &[ChatMessage],
    estimator: &dyn TokenEstimator,
    budget: &SummaryCompressionBudget,
    provider: &LlmProviderWrapper,
    request_config: &CompletionConfig,
) -> CompactResult<SummaryCompressionResult> {
    summarize_messages_with_previous(messages, None, estimator, budget, provider, request_config)
        .await
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
struct CompactSummarySource {
    covered_message_count: usize,
    previous_context: Vec<String>,
    new_context: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct CompactSummaryResponse {
    previous_context: Vec<String>,
    new_context: Vec<String>,
}

pub async fn summarize_messages_with_previous(
    messages: &[ChatMessage],
    previous_summary: Option<&str>,
    estimator: &dyn TokenEstimator,
    budget: &SummaryCompressionBudget,
    provider: &LlmProviderWrapper,
    request_config: &CompletionConfig,
) -> CompactResult<SummaryCompressionResult> {
    budget.validate()?;
    request_config
        .validate()
        .map_err(|error| CompactError::InvalidConfiguration {
            message: error.to_string(),
        })?;

    let rendered_new_lines = messages
        .iter()
        .map(|message| render_message_line(message, estimator, budget.max_summary_tokens))
        .collect::<Vec<_>>();
    let (new_lines, new_duplicate_lines) = normalize_and_dedupe_lines(rendered_new_lines);
    let (previous_lines, previous_duplicate_lines) =
        extract_previous_summary_lines(previous_summary);
    let removed_duplicate_lines = new_duplicate_lines + previous_duplicate_lines;

    if new_lines.is_empty() {
        return Err(CompactError::SummaryBudgetExhausted {
            message: format!(
                "max_summary_tokens={} is too small to fit the first message summary",
                budget.max_summary_tokens
            ),
        });
    }

    let request = build_summary_request(
        &CompactSummarySource {
            covered_message_count: messages.len(),
            previous_context: previous_lines.clone(),
            new_context: new_lines.clone(),
        },
        request_config,
    )?;
    let response = provider.complete(&request).await?;
    let compact_summary = parse_compact_summary_response(response.message.text.as_deref())?;
    let merged_lines = build_summary_lines(
        messages.len(),
        &normalize_model_lines(compact_summary.previous_context),
        &normalize_model_lines(compact_summary.new_context),
    );
    let mut compressed_lines =
        select_summary_lines(&merged_lines, estimator, budget.max_summary_tokens);

    let omitted_line_count = merged_lines.len().saturating_sub(compressed_lines.len());
    if omitted_line_count > 0 {
        let omission_notice = SummaryLine {
            text: format!("- Additional detail lines omitted: {omitted_line_count}"),
            kind: SummaryLineKind::OmissionNotice,
        };
        if summary_lines_fit(
            compressed_lines
                .iter()
                .cloned()
                .chain(std::iter::once(omission_notice.clone()))
                .collect::<Vec<_>>()
                .as_slice(),
            estimator,
            budget.max_summary_tokens,
        ) {
            compressed_lines.push(omission_notice);
        }
    }

    if !compressed_lines
        .iter()
        .any(|line| matches!(line.kind, SummaryLineKind::NewDetail))
    {
        return Err(CompactError::SummaryBudgetExhausted {
            message: format!(
                "max_summary_tokens={} is too small to fit the first message summary",
                budget.max_summary_tokens
            ),
        });
    }

    let summary = compressed_lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let estimated_tokens = estimator.estimate_text_tokens(&summary);

    Ok(SummaryCompressionResult {
        summary,
        covered_message_count: messages.len(),
        estimated_tokens,
        compressed_line_count: compressed_lines.len(),
        removed_duplicate_lines,
        omitted_line_count,
    })
}

fn build_summary_request(
    source: &CompactSummarySource,
    request_config: &CompletionConfig,
) -> CompactResult<LlmRequest> {
    let source_json =
        serde_json::to_string_pretty(source).map_err(|error| CompactError::SummaryParse {
            message: format!("failed to serialize compact summary source: {error}"),
        })?;
    let schema_hint = serde_json::to_string(&compact_summary_response_schema()).unwrap_or_default();
    let mut request = LlmRequest::new(vec![
        ChatMessage::system(
            "You compress conversation history into structured recall JSON. Use only the supplied source lines. Preserve concrete decisions, current state, failures, corrections, file references, and tool outcomes that still matter. Keep every string concise. Do not add markdown bullets or headings inside field values. Return ONLY a valid JSON object, no markdown fences.",
        ),
        ChatMessage::user(format!(
            "Summarize this compact source into structured JSON matching this schema:\n{schema_hint}\n\nSource:\n{source_json}"
        )),
    ]);
    request.max_tokens = Some(request_config.max_tokens);
    request.temperature = Some(request_config.temperature);
    request.response_format = ResponseFormat::JsonObject;
    Ok(request)
}

fn parse_compact_summary_response(text: Option<&str>) -> CompactResult<CompactSummaryResponse> {
    let text = text.ok_or_else(|| CompactError::SummaryParse {
        message: "summary provider returned no text payload".to_string(),
    })?;

    // Strip markdown code fences if present (```json ... ```)
    let cleaned = text.trim();
    let cleaned = cleaned
        .strip_prefix("```json")
        .or_else(|| cleaned.strip_prefix("```"))
        .unwrap_or(cleaned);
    let cleaned = cleaned.strip_suffix("```").unwrap_or(cleaned).trim();

    serde_json::from_str(cleaned).map_err(|error| CompactError::SummaryParse {
        message: format!("summary provider returned invalid JSON: {error}"),
    })
}

fn compact_summary_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "previous_context": string_array_schema(),
            "new_context": string_array_schema()
        },
        "required": ["previous_context", "new_context"]
    })
}

fn string_array_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "array",
        "items": {
            "type": "string"
        }
    })
}

fn extract_previous_summary_lines(previous_summary: Option<&str>) -> (Vec<String>, usize) {
    let Some(previous_summary) = previous_summary else {
        return (Vec::new(), 0);
    };

    let raw_lines = previous_summary
        .lines()
        .filter_map(|line| {
            let normalized = normalize_line(line);
            if normalized.is_empty()
                || normalized == "Conversation summary:"
                || normalized.starts_with("- Scope:")
                || normalized == "- Previously compacted context:"
                || normalized == "- Newly compacted context:"
                || normalized.starts_with("- Additional detail lines omitted:")
            {
                return None;
            }

            Some(strip_bullet_prefix(&normalized).to_string())
        })
        .collect::<Vec<_>>();

    normalize_and_dedupe_lines(raw_lines)
}

fn build_summary_lines(
    covered_message_count: usize,
    previous_lines: &[String],
    new_lines: &[String],
) -> Vec<SummaryLine> {
    let mut lines = vec![
        SummaryLine {
            text: "Conversation summary:".to_string(),
            kind: SummaryLineKind::Header,
        },
        SummaryLine {
            text: format!("- Scope: {covered_message_count} compacted message(s)."),
            kind: SummaryLineKind::Scope,
        },
    ];

    if !previous_lines.is_empty() {
        lines.push(SummaryLine {
            text: "- Previously compacted context:".to_string(),
            kind: SummaryLineKind::PreviousSectionHeader,
        });
        lines.extend(previous_lines.iter().cloned().map(|line| SummaryLine {
            text: format!("  - {line}"),
            kind: SummaryLineKind::PreviousDetail,
        }));
    }

    lines.push(SummaryLine {
        text: "- Newly compacted context:".to_string(),
        kind: SummaryLineKind::NewSectionHeader,
    });
    lines.extend(new_lines.iter().cloned().map(|line| SummaryLine {
        text: format!("  - {line}"),
        kind: SummaryLineKind::NewDetail,
    }));

    lines
}

fn normalize_and_dedupe_lines(lines: Vec<String>) -> (Vec<String>, usize) {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    let mut removed_duplicate_lines = 0usize;

    for line in lines {
        let normalized_line = normalize_line(&line);
        if normalized_line.is_empty() {
            continue;
        }

        if !seen.insert(normalized_line.to_ascii_lowercase()) {
            removed_duplicate_lines += 1;
            continue;
        }

        normalized.push(normalized_line);
    }

    (normalized, removed_duplicate_lines)
}

fn normalize_model_lines(lines: Vec<String>) -> Vec<String> {
    normalize_and_dedupe_lines(
        lines
            .into_iter()
            .map(|line| {
                line.trim()
                    .trim_start_matches("- ")
                    .trim_start_matches("* ")
                    .trim()
                    .to_string()
            })
            .collect(),
    )
    .0
}

fn select_summary_lines(
    lines: &[SummaryLine],
    estimator: &dyn TokenEstimator,
    max_summary_tokens: usize,
) -> Vec<SummaryLine> {
    let mut selected_indexes = Vec::new();

    for priority in 0..=3 {
        for (index, line) in lines.iter().enumerate() {
            if line_priority(line.kind) != priority || selected_indexes.contains(&index) {
                continue;
            }

            let candidate = selected_indexes
                .iter()
                .map(|selected_index| lines[*selected_index].clone())
                .chain(std::iter::once(line.clone()))
                .collect::<Vec<_>>();
            if summary_lines_fit(&candidate, estimator, max_summary_tokens) {
                selected_indexes.push(index);
            }
        }
    }

    selected_indexes.sort_unstable();
    selected_indexes
        .into_iter()
        .map(|index| lines[index].clone())
        .collect()
}

fn summary_lines_fit(
    lines: &[SummaryLine],
    estimator: &dyn TokenEstimator,
    max_summary_tokens: usize,
) -> bool {
    let candidate = lines
        .iter()
        .map(|line| line.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    estimator.estimate_text_tokens(&candidate) <= max_summary_tokens
}

fn line_priority(kind: SummaryLineKind) -> usize {
    match kind {
        SummaryLineKind::Header | SummaryLineKind::Scope => 0,
        SummaryLineKind::NewSectionHeader | SummaryLineKind::NewDetail => 1,
        SummaryLineKind::PreviousSectionHeader | SummaryLineKind::PreviousDetail => 2,
        SummaryLineKind::OmissionNotice => 3,
    }
}

fn normalize_line(line: &str) -> String {
    line.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_bullet_prefix(line: &str) -> &str {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .unwrap_or(line)
}

fn render_message_line(
    message: &ChatMessage,
    estimator: &dyn TokenEstimator,
    max_tokens: usize,
) -> String {
    let prefix = format!("[{}] ", message.role.as_str());
    let prefix_tokens = estimator.estimate_text_tokens(&prefix);
    if prefix_tokens >= max_tokens {
        return prefix.trim_end().to_string();
    }

    let content_budget = max_tokens.saturating_sub(prefix_tokens);
    let mut rendered_blocks = Vec::new();

    for block in &message.blocks {
        let block_budget = content_budget
            .saturating_sub(estimator.estimate_text_tokens(&rendered_blocks.join(" | ")));
        if block_budget == 0 {
            break;
        }

        let rendered_block = render_block(block, estimator, block_budget);
        let candidate_blocks = if rendered_blocks.is_empty() {
            rendered_block.clone()
        } else {
            format!("{} | {}", rendered_blocks.join(" | "), rendered_block)
        };

        if estimator.estimate_text_tokens(&candidate_blocks) > content_budget {
            break;
        }

        rendered_blocks.push(rendered_block);
    }

    if rendered_blocks.is_empty() {
        prefix.trim_end().to_string()
    } else {
        format!("{prefix}{}", rendered_blocks.join(" | "))
    }
}

fn render_block(block: &ContentBlock, estimator: &dyn TokenEstimator, max_tokens: usize) -> String {
    match block {
        ContentBlock::Text { text } => fit_text_to_token_budget(text, estimator, max_tokens),
        ContentBlock::ToolUse {
            call_id,
            tool_name,
            input,
        } => {
            let prefix = format!("tool_use:{tool_name}:{call_id}:");
            let prefix_tokens = estimator.estimate_text_tokens(&prefix);
            let payload = fit_text_to_token_budget(
                &input.to_string(),
                estimator,
                max_tokens.saturating_sub(prefix_tokens),
            );
            format!("{prefix}{payload}")
        }
        ContentBlock::ToolResult {
            call_id,
            tool_name,
            output,
            is_error,
        } => {
            let prefix = format!("tool_result:{tool_name}:{call_id}:{is_error}:");
            let prefix_tokens = estimator.estimate_text_tokens(&prefix);
            let payload = fit_text_to_token_budget(
                output,
                estimator,
                max_tokens.saturating_sub(prefix_tokens),
            );
            format!("{prefix}{payload}")
        }
        ContentBlock::Image { description } => {
            let prefix = "image:".to_string();
            let prefix_tokens = estimator.estimate_text_tokens(&prefix);
            let payload = fit_text_to_token_budget(
                description,
                estimator,
                max_tokens.saturating_sub(prefix_tokens),
            );
            format!("{prefix}{payload}")
        }
        ContentBlock::Document { description } => {
            let prefix = "document:".to_string();
            let prefix_tokens = estimator.estimate_text_tokens(&prefix);
            let payload = fit_text_to_token_budget(
                description,
                estimator,
                max_tokens.saturating_sub(prefix_tokens),
            );
            format!("{prefix}{payload}")
        }
    }
}

fn fit_text_to_token_budget(
    text: &str,
    estimator: &dyn TokenEstimator,
    max_tokens: usize,
) -> String {
    if max_tokens == 0 || text.is_empty() {
        return String::new();
    }

    let mut accepted_words = Vec::new();
    for word in text.split_whitespace() {
        let candidate = if accepted_words.is_empty() {
            word.to_string()
        } else {
            format!("{} {}", accepted_words.join(" "), word)
        };

        if estimator.estimate_text_tokens(&candidate) > max_tokens {
            break;
        }

        accepted_words.push(word.to_string());
    }

    if !accepted_words.is_empty() {
        return accepted_words.join(" ");
    }

    let mut accepted_characters = String::new();
    for character in text.chars() {
        let mut candidate = accepted_characters.clone();
        candidate.push(character);
        if estimator.estimate_text_tokens(&candidate) > max_tokens {
            break;
        }
        accepted_characters = candidate;
    }

    accepted_characters
}
