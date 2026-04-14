use std::sync::Arc;

use agent_contracts::TokenEstimator;
use agent_llm::{ChatMessageExt, CompletionConfigExt, LlmRequestExt, ResponseFormatExt};
use agent_types::{ChatMessage, CompletionConfig, LlmRequest, MessageRole, ResponseFormat};
use llm_client::LlmProviderWrapper;
use serde::{Deserialize, Serialize};

use crate::{
    ensure_valid_session_id, invalid_session_id_io_error, MemoryError, MemoryResult,
    MemorySnapshot, SessionMemoryStore,
};

const NONE_LINE: &str = "<none>";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMemoryPolicy {
    pub summary_message_limit: usize,
    pub summary_instruction_limit: usize,
    pub summary_fact_limit: usize,
    pub summary_prompt_history_limit: usize,
    pub max_section_tokens: usize,
    pub max_total_tokens: usize,
}

impl SessionMemoryPolicy {
    pub fn validate(&self) -> MemoryResult<()> {
        if self.summary_message_limit == 0
            || self.summary_instruction_limit == 0
            || self.summary_fact_limit == 0
            || self.summary_prompt_history_limit == 0
            || self.max_section_tokens == 0
            || self.max_total_tokens == 0
        {
            return Err(MemoryError::InvalidConfiguration {
                message: "session memory policy limits must be greater than zero".to_string(),
            });
        }

        if self.max_total_tokens < self.max_section_tokens {
            return Err(MemoryError::InvalidConfiguration {
                message:
                    "session memory policy max_total_tokens must be greater than or equal to max_section_tokens"
                        .to_string(),
            });
        }

        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionMemorySummary {
    pub session_id: String,
    pub summary: String,
    pub updated_at: u64,
    pub message_count: usize,
    pub fact_count: usize,
    pub summarized_through_message_id: Option<String>,
    pub summarized_through_timestamp_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionMemoryBudgetingResult {
    pub summary: String,
    pub truncated_sections: Vec<String>,
    pub total_budget_applied: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SessionSummarySource {
    pub session_id: String,
    pub message_count: usize,
    pub fact_count: usize,
    pub current_state: Vec<String>,
    pub user_intent: Vec<String>,
    pub files_and_references: Vec<String>,
    pub errors_and_corrections: Vec<String>,
    pub pending_tasks: Vec<String>,
    pub current_work: Vec<String>,
    pub next_step: Vec<String>,
    pub task_specification: Vec<String>,
    pub facts: Vec<String>,
    pub prompt_history: Vec<String>,
    pub recent_conversation: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
struct SessionSummaryResponse {
    current_state: Vec<String>,
    user_intent: Vec<String>,
    files_and_references: Vec<String>,
    errors_and_corrections: Vec<String>,
    pending_tasks: Vec<String>,
    current_work: Vec<String>,
    next_step: Vec<String>,
    task_specification: Vec<String>,
    facts: Vec<String>,
    prompt_history: Vec<String>,
    recent_conversation: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SessionSection {
    title: String,
    lines: Vec<String>,
    priority: SectionPriority,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SectionPriority {
    Low,
    Medium,
    High,
    Critical,
}

pub struct SessionMemoryManager {
    store: Arc<dyn SessionMemoryStore>,
    estimator: Arc<dyn TokenEstimator>,
    policy: SessionMemoryPolicy,
    summary_provider: Arc<LlmProviderWrapper>,
    summary_request: CompletionConfig,
}

impl SessionMemoryManager {
    pub fn new(
        store: Arc<dyn SessionMemoryStore>,
        estimator: Arc<dyn TokenEstimator>,
        policy: SessionMemoryPolicy,
        summary_provider: Arc<LlmProviderWrapper>,
        summary_request: CompletionConfig,
    ) -> MemoryResult<Self> {
        policy.validate()?;
        summary_request
            .validate()
            .map_err(|error: String| MemoryError::InvalidConfiguration { message: error })?;
        Ok(Self {
            store,
            estimator,
            policy,
            summary_provider,
            summary_request,
        })
    }

    pub async fn build_summary(
        &self,
        snapshot: &MemorySnapshot,
        updated_at: u64,
    ) -> MemoryResult<SessionMemorySummary> {
        ensure_valid_session_id(&snapshot.session_id)?;

        let source = build_summary_source(snapshot, &self.policy);
        let request = self.build_summary_request(&source)?;
        let response = self.summary_provider.complete(&request).await?;
        let llm_sections = parse_session_summary_response(response.message.text.as_deref())?;
        let sections = llm_sections.into_sections(snapshot);
        let budgeted = budget_session_sections(
            sections,
            self.estimator.as_ref(),
            self.policy.max_section_tokens,
            self.policy.max_total_tokens,
        )?;

        Ok(SessionMemorySummary {
            session_id: snapshot.session_id.clone(),
            summary: budgeted.summary,
            updated_at,
            message_count: snapshot.messages.len(),
            fact_count: snapshot.facts.len(),
            summarized_through_message_id: snapshot
                .messages
                .last()
                .and_then(|message| message.message_id.clone()),
            summarized_through_timestamp_ms: snapshot
                .messages
                .last()
                .map(|message| message.timestamp_ms),
        })
    }

    pub async fn persist_summary(&self, summary: &SessionMemorySummary) -> std::io::Result<()> {
        ensure_valid_session_id(&summary.session_id).map_err(|_| invalid_session_id_io_error())?;
        self.store.save_summary(summary).await
    }

    pub async fn load_summary(&self, session_id: &str) -> std::io::Result<SessionMemorySummary> {
        ensure_valid_session_id(session_id).map_err(|_| invalid_session_id_io_error())?;
        self.store.load_summary(session_id).await
    }

    fn build_summary_request(&self, source: &SessionSummarySource) -> MemoryResult<LlmRequest> {
        let source_json = serde_json::to_string_pretty(source).map_err(|error| {
            MemoryError::SessionMemorySummaryParse {
                message: format!("failed to serialize session summary source: {error}"),
            }
        })?;
        let mut request = LlmRequest::new(vec![
            ChatMessage::text(
                MessageRole::System,
                "You produce structured session memory for an agent runtime. Use only the supplied source data. Keep each field concise, concrete, and recall-oriented. Preserve file paths, identifiers, unresolved tasks, failures, corrections, and the next action whenever they appear. Return JSON only. Do not add markdown headings or bullet prefixes inside field values.",
                0,
            ),
            ChatMessage::text(
                MessageRole::User,
                format!("Summarize this session snapshot into structured memory JSON.\n\nSource:\n{source_json}"),
                0,
            ),
        ]);
        request.max_tokens = Some(self.summary_request.max_tokens);
        request.temperature = Some(self.summary_request.temperature);
        request.response_format = ResponseFormat::json_schema(
            "session_memory_summary",
            session_summary_response_schema(),
        );
        Ok(request)
    }
}

pub fn truncate_session_memory_summary(
    summary: &str,
    estimator: &dyn TokenEstimator,
    max_section_tokens: usize,
    max_total_tokens: usize,
) -> MemoryResult<SessionMemoryBudgetingResult> {
    if max_section_tokens == 0 || max_total_tokens == 0 {
        return Err(MemoryError::InvalidConfiguration {
            message:
                "session memory truncation budgets max_section_tokens and max_total_tokens must be greater than zero"
                    .to_string(),
        });
    }

    if max_total_tokens < max_section_tokens {
        return Err(MemoryError::InvalidConfiguration {
            message:
                "session memory truncation max_total_tokens must be greater than or equal to max_section_tokens"
                    .to_string(),
        });
    }

    let sections = parse_summary_sections(summary);
    budget_session_sections(sections, estimator, max_section_tokens, max_total_tokens)
}

fn build_summary_sections(
    snapshot: &MemorySnapshot,
    policy: &SessionMemoryPolicy,
) -> Vec<SessionSection> {
    let recent_messages = snapshot
        .conversation
        .iter()
        .rev()
        .take(policy.summary_message_limit)
        .map(render_conversation_message)
        .collect::<Vec<_>>();
    let instruction_lines = snapshot
        .instructions
        .iter()
        .rev()
        .take(policy.summary_instruction_limit)
        .map(|instruction| format!("{}: {}", instruction.source, instruction.content))
        .collect::<Vec<_>>();
    let fact_lines = snapshot
        .facts
        .iter()
        .rev()
        .take(policy.summary_fact_limit)
        .map(|fact| format!("{}: {}", fact.key, fact.content))
        .collect::<Vec<_>>();
    let prompt_history_lines = snapshot
        .prompt_history
        .iter()
        .rev()
        .take(policy.summary_prompt_history_limit)
        .map(|entry| format!("{}: {}", entry.recorded_at, entry.prompt))
        .collect::<Vec<_>>();
    let user_intent_lines = dedupe_preserve_order(
        snapshot
            .conversation
            .iter()
            .rev()
            .filter(|message| matches!(message.role, crate::MemoryRole::User))
            .take(policy.summary_message_limit)
            .map(render_conversation_message)
            .chain(prompt_history_lines.iter().cloned())
            .collect(),
    );
    let file_reference_lines = collect_file_references(&[
        &instruction_lines,
        &fact_lines,
        &prompt_history_lines,
        &recent_messages,
    ]);
    let error_lines = dedupe_preserve_order(
        instruction_lines
            .iter()
            .chain(fact_lines.iter())
            .chain(recent_messages.iter())
            .filter(|line| contains_error_signal(line))
            .cloned()
            .collect(),
    );
    let pending_task_lines = snapshot
        .task
        .as_ref()
        .map(|task| {
            if task.pending_steps.is_empty() {
                vec![NONE_LINE.to_string()]
            } else {
                task.pending_steps.clone()
            }
        })
        .unwrap_or_else(|| vec![NONE_LINE.to_string()]);
    let current_work_lines = dedupe_preserve_order(
        snapshot
            .conversation
            .iter()
            .rev()
            .filter(|message| {
                matches!(
                    message.role,
                    crate::MemoryRole::User | crate::MemoryRole::Assistant
                )
            })
            .take(policy.summary_message_limit)
            .map(render_conversation_message)
            .collect(),
    );
    let next_step_lines = build_next_step_lines(snapshot, &user_intent_lines);
    let current_state_lines = build_current_state_lines(snapshot);

    vec![
        SessionSection {
            title: "# Session".to_string(),
            lines: vec![
                format!("Session ID: {}", snapshot.session_id),
                format!("Message Count: {}", snapshot.messages.len()),
                format!("Fact Count: {}", snapshot.facts.len()),
            ],
            priority: SectionPriority::Critical,
        },
        SessionSection {
            title: "# Current State".to_string(),
            lines: current_state_lines,
            priority: SectionPriority::Critical,
        },
        SessionSection {
            title: "# User Intent".to_string(),
            lines: ensure_non_empty(user_intent_lines),
            priority: SectionPriority::High,
        },
        SessionSection {
            title: "# Files and References".to_string(),
            lines: ensure_non_empty(file_reference_lines),
            priority: SectionPriority::High,
        },
        SessionSection {
            title: "# Errors and Corrections".to_string(),
            lines: ensure_non_empty(error_lines),
            priority: SectionPriority::High,
        },
        SessionSection {
            title: "# Pending Tasks".to_string(),
            lines: ensure_non_empty(pending_task_lines),
            priority: SectionPriority::High,
        },
        SessionSection {
            title: "# Current Work".to_string(),
            lines: ensure_non_empty(current_work_lines),
            priority: SectionPriority::High,
        },
        SessionSection {
            title: "# Next Step".to_string(),
            lines: ensure_non_empty(next_step_lines),
            priority: SectionPriority::Critical,
        },
        SessionSection {
            title: "# Task Specification".to_string(),
            lines: ensure_non_empty(instruction_lines),
            priority: SectionPriority::Medium,
        },
        SessionSection {
            title: "# Facts".to_string(),
            lines: ensure_non_empty(fact_lines),
            priority: SectionPriority::Medium,
        },
        SessionSection {
            title: "# Prompt History".to_string(),
            lines: ensure_non_empty(prompt_history_lines),
            priority: SectionPriority::Low,
        },
        SessionSection {
            title: "# Recent Conversation".to_string(),
            lines: ensure_non_empty(recent_messages),
            priority: SectionPriority::Low,
        },
    ]
}

pub fn build_summary_source(
    snapshot: &MemorySnapshot,
    policy: &SessionMemoryPolicy,
) -> SessionSummarySource {
    let sections = build_summary_sections(snapshot, policy);
    SessionSummarySource {
        session_id: snapshot.session_id.clone(),
        message_count: snapshot.messages.len(),
        fact_count: snapshot.facts.len(),
        current_state: section_lines(&sections, "# Current State"),
        user_intent: section_lines(&sections, "# User Intent"),
        files_and_references: section_lines(&sections, "# Files and References"),
        errors_and_corrections: section_lines(&sections, "# Errors and Corrections"),
        pending_tasks: section_lines(&sections, "# Pending Tasks"),
        current_work: section_lines(&sections, "# Current Work"),
        next_step: section_lines(&sections, "# Next Step"),
        task_specification: section_lines(&sections, "# Task Specification"),
        facts: section_lines(&sections, "# Facts"),
        prompt_history: section_lines(&sections, "# Prompt History"),
        recent_conversation: section_lines(&sections, "# Recent Conversation"),
    }
}

fn budget_session_sections(
    sections: Vec<SessionSection>,
    estimator: &dyn TokenEstimator,
    max_section_tokens: usize,
    max_total_tokens: usize,
) -> MemoryResult<SessionMemoryBudgetingResult> {
    let mut budgeted_sections = sections
        .into_iter()
        .map(|section| apply_section_budget(section, estimator, max_section_tokens))
        .collect::<Vec<_>>();

    let mut total_budget_applied = false;
    while estimate_sections_tokens(&budgeted_sections, estimator) > max_total_tokens {
        total_budget_applied = true;
        let mut changed = false;

        for priority in [
            SectionPriority::Low,
            SectionPriority::Medium,
            SectionPriority::High,
            SectionPriority::Critical,
        ] {
            for index in 0..budgeted_sections.len() {
                if budgeted_sections[index].priority != priority {
                    continue;
                }

                if shrink_section_for_total_budget(
                    &mut budgeted_sections[index],
                    estimator,
                    max_total_tokens,
                ) {
                    changed = true;
                    if estimate_sections_tokens(&budgeted_sections, estimator) <= max_total_tokens {
                        break;
                    }
                }
            }

            if estimate_sections_tokens(&budgeted_sections, estimator) <= max_total_tokens {
                break;
            }
        }

        if !changed {
            return Err(MemoryError::SessionMemoryBudgetExhausted {
                message: format!(
                    "max_total_tokens={} is too small to fit the minimum session memory sections",
                    max_total_tokens
                ),
            });
        }
    }

    Ok(SessionMemoryBudgetingResult {
        summary: render_budgeted_sections(&budgeted_sections),
        truncated_sections: budgeted_sections
            .iter()
            .filter(|section| section.truncated)
            .map(|section| section.title.clone())
            .collect(),
        total_budget_applied,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BudgetedSection {
    title: String,
    lines: Vec<String>,
    priority: SectionPriority,
    truncated: bool,
}

fn apply_section_budget(
    section: SessionSection,
    estimator: &dyn TokenEstimator,
    max_section_tokens: usize,
) -> BudgetedSection {
    let reminder = format!(
        "Reminder: section exceeded max_section_tokens={}; trailing detail omitted.",
        max_section_tokens
    );
    let mut kept_lines = Vec::new();

    for line in ensure_non_empty(section.lines.clone()) {
        let candidate = kept_lines
            .iter()
            .cloned()
            .chain(std::iter::once(line.clone()))
            .collect::<Vec<_>>();
        if estimate_section_tokens(&section.title, &candidate, estimator) <= max_section_tokens {
            kept_lines = candidate;
            continue;
        }

        if kept_lines.is_empty() {
            let fitted = fit_line_to_budget(
                &line,
                estimator,
                remaining_section_budget(&section.title, &[], estimator, max_section_tokens),
            );
            if !fitted.is_empty() {
                kept_lines.push(fitted);
            }
        }
        break;
    }

    let original_lines = ensure_non_empty(section.lines);
    let truncated = kept_lines != original_lines;
    if truncated {
        push_notice_line_with_budget(
            &section.title,
            &mut kept_lines,
            &reminder,
            estimator,
            max_section_tokens,
        );
    }

    BudgetedSection {
        title: section.title,
        lines: ensure_non_empty(kept_lines),
        priority: section.priority,
        truncated,
    }
}

fn shrink_section_for_total_budget(
    section: &mut BudgetedSection,
    _estimator: &dyn TokenEstimator,
    max_total_tokens: usize,
) -> bool {
    if section.lines.len() > 1 {
        section.lines.pop();
        section.truncated = true;
        return true;
    }

    let reminder = format!(
        "Reminder: section condensed to satisfy max_total_tokens={}.",
        max_total_tokens
    );
    if section.lines.first().map(|line| line.as_str()) != Some(reminder.as_str()) {
        section.lines = vec![reminder];
        section.truncated = true;
        return true;
    }

    false
}

fn parse_summary_sections(summary: &str) -> Vec<SessionSection> {
    let mut sections = Vec::new();
    let mut current_title: Option<String> = None;
    let mut current_lines = Vec::new();

    for line in summary.lines() {
        if line.starts_with("# ") {
            if let Some(title) = current_title.take() {
                sections.push(SessionSection {
                    priority: section_priority(&title),
                    title,
                    lines: ensure_non_empty(
                        current_lines
                            .drain(..)
                            .map(strip_summary_bullet_prefix)
                            .collect(),
                    ),
                });
            }
            current_title = Some(line.to_string());
        } else if !line.trim().is_empty() {
            current_lines.push(line.to_string());
        }
    }

    if let Some(title) = current_title {
        sections.push(SessionSection {
            priority: section_priority(&title),
            title,
            lines: ensure_non_empty(
                current_lines
                    .into_iter()
                    .map(strip_summary_bullet_prefix)
                    .collect(),
            ),
        });
    }

    sections
}

fn build_current_state_lines(snapshot: &MemorySnapshot) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(task) = &snapshot.task {
        lines.push(format!("Current Task: {}", task.current_task));
        if task.pending_steps.is_empty() {
            lines.push("Pending Steps: <none>".to_string());
        } else {
            lines.push(format!("Pending Steps: {}", task.pending_steps.join(" | ")));
        }
        lines.push(format!(
            "Legacy Task State: task={}, pending_steps={}",
            task.current_task,
            if task.pending_steps.is_empty() {
                NONE_LINE.to_string()
            } else {
                task.pending_steps.join(" | ")
            }
        ));
    } else {
        lines.push("Current Task: <none>".to_string());
        lines.push("Pending Steps: <none>".to_string());
    }

    if let Some(baseline) = &snapshot.usage_baseline {
        lines.push(format!(
            "Usage Baseline: estimated_history_tokens={}, last_prompt_tokens={}, last_completion_tokens={}, recorded_at={}",
            baseline.estimated_history_tokens,
            baseline.last_prompt_tokens,
            baseline.last_completion_tokens,
            baseline.recorded_at
        ));
    } else {
        lines.push("Usage Baseline: <none>".to_string());
    }

    lines
}

impl SessionSummaryResponse {
    fn into_sections(self, snapshot: &MemorySnapshot) -> Vec<SessionSection> {
        vec![
            SessionSection {
                title: "# Session".to_string(),
                lines: vec![
                    format!("Session ID: {}", snapshot.session_id),
                    format!("Message Count: {}", snapshot.messages.len()),
                    format!("Fact Count: {}", snapshot.facts.len()),
                ],
                priority: SectionPriority::Critical,
            },
            SessionSection {
                title: "# Current State".to_string(),
                lines: normalize_model_lines(self.current_state),
                priority: SectionPriority::Critical,
            },
            SessionSection {
                title: "# User Intent".to_string(),
                lines: normalize_model_lines(self.user_intent),
                priority: SectionPriority::High,
            },
            SessionSection {
                title: "# Files and References".to_string(),
                lines: normalize_model_lines(self.files_and_references),
                priority: SectionPriority::High,
            },
            SessionSection {
                title: "# Errors and Corrections".to_string(),
                lines: normalize_model_lines(self.errors_and_corrections),
                priority: SectionPriority::High,
            },
            SessionSection {
                title: "# Pending Tasks".to_string(),
                lines: normalize_model_lines(self.pending_tasks),
                priority: SectionPriority::High,
            },
            SessionSection {
                title: "# Current Work".to_string(),
                lines: normalize_model_lines(self.current_work),
                priority: SectionPriority::High,
            },
            SessionSection {
                title: "# Next Step".to_string(),
                lines: normalize_model_lines(self.next_step),
                priority: SectionPriority::Critical,
            },
            SessionSection {
                title: "# Task Specification".to_string(),
                lines: normalize_model_lines(self.task_specification),
                priority: SectionPriority::Medium,
            },
            SessionSection {
                title: "# Facts".to_string(),
                lines: normalize_model_lines(self.facts),
                priority: SectionPriority::Medium,
            },
            SessionSection {
                title: "# Prompt History".to_string(),
                lines: normalize_model_lines(self.prompt_history),
                priority: SectionPriority::Low,
            },
            SessionSection {
                title: "# Recent Conversation".to_string(),
                lines: normalize_model_lines(self.recent_conversation),
                priority: SectionPriority::Low,
            },
        ]
    }
}

fn parse_session_summary_response(text: Option<&str>) -> MemoryResult<SessionSummaryResponse> {
    let text = text.ok_or_else(|| MemoryError::SessionMemorySummaryParse {
        message: "summary provider returned no text payload".to_string(),
    })?;

    serde_json::from_str(text).map_err(|error| MemoryError::SessionMemorySummaryParse {
        message: format!("summary provider returned invalid JSON: {error}"),
    })
}

fn normalize_model_lines(lines: Vec<String>) -> Vec<String> {
    ensure_non_empty(dedupe_preserve_order(
        lines
            .into_iter()
            .map(|line| {
                line.trim()
                    .trim_start_matches("- ")
                    .trim_start_matches("* ")
                    .trim()
                    .to_string()
            })
            .filter(|line| !line.is_empty())
            .collect(),
    ))
}

fn section_lines(sections: &[SessionSection], title: &str) -> Vec<String> {
    sections
        .iter()
        .find(|section| section.title == title)
        .map(|section| section.lines.clone())
        .unwrap_or_default()
}

fn session_summary_response_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "current_state": string_array_schema(),
            "user_intent": string_array_schema(),
            "files_and_references": string_array_schema(),
            "errors_and_corrections": string_array_schema(),
            "pending_tasks": string_array_schema(),
            "current_work": string_array_schema(),
            "next_step": string_array_schema(),
            "task_specification": string_array_schema(),
            "facts": string_array_schema(),
            "prompt_history": string_array_schema(),
            "recent_conversation": string_array_schema()
        },
        "required": [
            "current_state",
            "user_intent",
            "files_and_references",
            "errors_and_corrections",
            "pending_tasks",
            "current_work",
            "next_step",
            "task_specification",
            "facts",
            "prompt_history",
            "recent_conversation"
        ]
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

fn build_next_step_lines(snapshot: &MemorySnapshot, user_intent_lines: &[String]) -> Vec<String> {
    if let Some(task) = &snapshot.task {
        if let Some(step) = task.pending_steps.first() {
            return vec![step.clone()];
        }
    }

    if let Some(intent) = user_intent_lines.first() {
        return vec![intent.clone()];
    }

    vec![NONE_LINE.to_string()]
}

fn collect_file_references(line_groups: &[&[String]]) -> Vec<String> {
    let mut files = Vec::new();
    for lines in line_groups {
        for line in *lines {
            for token in line.split_whitespace() {
                let candidate = trim_reference_token(token);
                if looks_like_file_reference(candidate) {
                    files.push(candidate.to_string());
                }
            }
        }
    }

    dedupe_preserve_order(files)
}

fn contains_error_signal(line: &str) -> bool {
    let lowercase = line.to_ascii_lowercase();
    ["error", "fail", "failed", "invalid", "fix", "correct"]
        .iter()
        .any(|keyword| lowercase.contains(keyword))
}

fn looks_like_file_reference(token: &str) -> bool {
    token == "Cargo.toml"
        || token.contains('/')
        || [
            ".rs", ".md", ".toml", ".json", ".yaml", ".yml", ".ts", ".tsx", ".js", ".jsx", ".py",
            ".sh",
        ]
        .iter()
        .any(|suffix| token.ends_with(suffix))
}

fn trim_reference_token(token: &str) -> &str {
    token.trim_matches(|character: char| {
        matches!(
            character,
            ',' | '.' | ';' | ':' | '(' | ')' | '[' | ']' | '{' | '}' | '"' | '\''
        )
    })
}

fn dedupe_preserve_order(lines: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut deduped = Vec::new();

    for line in lines {
        let normalized = line.trim();
        if normalized.is_empty() {
            continue;
        }

        if seen.insert(normalized.to_ascii_lowercase()) {
            deduped.push(normalized.to_string());
        }
    }

    deduped
}

fn ensure_non_empty(lines: Vec<String>) -> Vec<String> {
    if lines.is_empty() {
        vec![NONE_LINE.to_string()]
    } else {
        lines
    }
}

fn section_priority(title: &str) -> SectionPriority {
    match title {
        "# Session" | "# Current State" | "# Next Step" => SectionPriority::Critical,
        "# User Intent"
        | "# Files and References"
        | "# Errors and Corrections"
        | "# Pending Tasks"
        | "# Current Work" => SectionPriority::High,
        "# Task Specification" | "# Facts" => SectionPriority::Medium,
        _ => SectionPriority::Low,
    }
}

fn render_budgeted_sections(sections: &[BudgetedSection]) -> String {
    sections
        .iter()
        .map(|section| render_section(&section.title, &section.lines))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn render_section(title: &str, lines: &[String]) -> String {
    let content = ensure_non_empty(lines.to_vec())
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("{title}\n{content}")
}

fn estimate_sections_tokens(sections: &[BudgetedSection], estimator: &dyn TokenEstimator) -> usize {
    estimator.estimate_text_tokens(&render_budgeted_sections(sections))
}

fn estimate_section_tokens(title: &str, lines: &[String], estimator: &dyn TokenEstimator) -> usize {
    estimator.estimate_text_tokens(&render_section(title, lines))
}

fn remaining_section_budget(
    title: &str,
    lines: &[String],
    estimator: &dyn TokenEstimator,
    max_section_tokens: usize,
) -> usize {
    max_section_tokens.saturating_sub(estimate_section_tokens(title, lines, estimator))
}

fn push_notice_line_with_budget(
    title: &str,
    lines: &mut Vec<String>,
    notice: &str,
    estimator: &dyn TokenEstimator,
    max_section_tokens: usize,
) {
    let fitted_notice = fit_line_to_budget(
        notice,
        estimator,
        remaining_section_budget(title, lines, estimator, max_section_tokens),
    );

    if fitted_notice.is_empty() {
        return;
    }

    let candidate = lines
        .iter()
        .cloned()
        .chain(std::iter::once(fitted_notice.clone()))
        .collect::<Vec<_>>();
    if estimate_section_tokens(title, &candidate, estimator) <= max_section_tokens {
        lines.push(fitted_notice);
    }
}

fn fit_line_to_budget(text: &str, estimator: &dyn TokenEstimator, max_tokens: usize) -> String {
    if max_tokens == 0 {
        return String::new();
    }

    if estimator.estimate_text_tokens(text) <= max_tokens {
        return text.to_string();
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

fn strip_summary_bullet_prefix(line: String) -> String {
    line.strip_prefix("- ").map(str::to_string).unwrap_or(line)
}

fn render_conversation_message(message: &crate::ConversationMessage) -> String {
    let role = match message.role {
        crate::MemoryRole::System => "system",
        crate::MemoryRole::User => "user",
        crate::MemoryRole::Assistant => "assistant",
        crate::MemoryRole::Tool => "tool",
    };

    let blocks = message
        .blocks
        .iter()
        .map(|block| match block {
            crate::ContentBlock::Text { text } => text.clone(),
            crate::ContentBlock::ToolUse {
                call_id,
                tool_name,
                input,
            } => format!("tool_use:{tool_name}:{call_id}:{input}"),
            crate::ContentBlock::ToolResult {
                call_id,
                tool_name,
                output,
                is_error,
            } => format!("tool_result:{tool_name}:{call_id}:{is_error}:{output}"),
            crate::ContentBlock::Image { description } => format!("image:{description}"),
            crate::ContentBlock::Document { description } => format!("document:{description}"),
        })
        .collect::<Vec<_>>()
        .join(" | ");

    format!("[{role}] {blocks}")
}
