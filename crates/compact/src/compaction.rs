use std::collections::HashSet;

use agent_types::CompressionMeta;
use agent_types::{ChatMessage, ContentBlock, MessageRole};
use serde::{Deserialize, Serialize};

use crate::{CompactError, CompactResult};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum CompactMode {
    HistorySnip,
    ContextCollapse,
    AutoCompact,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum PartialDirection {
    BeforePivot,
    AfterPivot,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionBoundary {
    pub pivot_message_id: String,
    pub direction: PartialDirection,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompactRequest {
    pub messages: Vec<ChatMessage>,
    pub mode: CompactMode,
    pub boundary: Option<CompactionBoundary>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompactionDecision {
    pub mode: CompactMode,
    pub boundary: Option<CompactionBoundary>,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CompactionResult {
    pub messages: Vec<ChatMessage>,
    pub summary: Option<String>,
    pub boundary: Option<CompactionBoundary>,
    pub updated_meta: CompressionMeta,
    pub estimated_tokens: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompactionWindow {
    start: usize,
    end: usize,
}

pub fn discover_boundary(
    messages: &[ChatMessage],
    preserve_tail_messages: usize,
) -> Option<CompactionBoundary> {
    let boundary_search_end = messages.len().saturating_sub(preserve_tail_messages);
    messages
        .iter()
        .take(boundary_search_end)
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            let pivot_message_id = message.message_id.clone()?;
            let has_removable_history = messages
                .iter()
                .take(index)
                .any(|message| !matches!(message.role, MessageRole::System));

            has_removable_history.then_some(CompactionBoundary {
                pivot_message_id,
                direction: PartialDirection::BeforePivot,
            })
        })
}

pub fn candidate_indexes(
    messages: &[ChatMessage],
    preserve_tail_messages: usize,
    boundary: Option<&CompactionBoundary>,
) -> CompactResult<Vec<usize>> {
    let window = resolve_compaction_window(messages, preserve_tail_messages, boundary)?;
    let adjusted_end = adjust_keep_start_for_invariants(messages, window.end, window.start);

    // The first user message carries the original task instructions and must never
    // be compacted away — it is the only message that tells the model what to do.
    let first_user_index = messages
        .iter()
        .position(|m| matches!(m.role, MessageRole::User));

    Ok(messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            if matches!(message.role, MessageRole::System) {
                return None;
            }

            if Some(index) == first_user_index {
                return None;
            }

            if index < window.start || index >= adjusted_end {
                return None;
            }

            Some(index)
        })
        .collect())
}

pub fn apply_candidate_replacement(
    messages: &[ChatMessage],
    candidate_indexes: &[usize],
    replacement: Option<ChatMessage>,
) -> Vec<ChatMessage> {
    let removable = candidate_indexes.iter().copied().collect::<HashSet<_>>();
    let first_candidate = candidate_indexes.iter().copied().min();
    let mut result = Vec::new();

    for (index, message) in messages.iter().enumerate() {
        if Some(index) == first_candidate {
            if let Some(replacement) = &replacement {
                result.push(replacement.clone());
            }
        }

        if !removable.contains(&index) {
            result.push(message.clone());
        }
    }

    result
}

fn resolve_boundary_index(
    messages: &[ChatMessage],
    boundary: &CompactionBoundary,
) -> CompactResult<usize> {
    messages
        .iter()
        .position(|message| {
            message.message_id.as_deref() == Some(boundary.pivot_message_id.as_str())
        })
        .ok_or_else(|| CompactError::BoundaryNotFound {
            pivot_message_id: boundary.pivot_message_id.clone(),
        })
}

fn resolve_compaction_window(
    messages: &[ChatMessage],
    preserve_tail_messages: usize,
    boundary: Option<&CompactionBoundary>,
) -> CompactResult<CompactionWindow> {
    let protected_tail_start = messages.len().saturating_sub(preserve_tail_messages);

    let window = match boundary {
        Some(boundary) => {
            let boundary_index = resolve_boundary_index(messages, boundary)?;
            match boundary.direction {
                PartialDirection::BeforePivot => CompactionWindow {
                    start: 0,
                    end: boundary_index,
                },
                PartialDirection::AfterPivot => CompactionWindow {
                    start: boundary_index.saturating_add(1),
                    end: protected_tail_start,
                },
            }
        }
        None => CompactionWindow {
            start: 0,
            end: protected_tail_start,
        },
    };

    Ok(CompactionWindow {
        start: window.start.min(messages.len()),
        end: window.end.min(messages.len()),
    })
}

pub fn adjust_keep_start_for_invariants(
    messages: &[ChatMessage],
    keep_start: usize,
    floor_start: usize,
) -> usize {
    if floor_start >= keep_start || keep_start >= messages.len() {
        return keep_start.min(messages.len());
    }

    let mut adjusted_keep_start = keep_start;

    loop {
        let required_keep_start =
            find_required_keep_start(messages, floor_start, adjusted_keep_start);
        if required_keep_start == adjusted_keep_start {
            return adjusted_keep_start;
        }
        adjusted_keep_start = required_keep_start;
    }
}

fn find_required_keep_start(
    messages: &[ChatMessage],
    window_start: usize,
    keep_start: usize,
) -> usize {
    let mut required_keep_start = keep_start;
    let kept_messages = &messages[keep_start..];
    let required_tool_use_ids = kept_messages
        .iter()
        .flat_map(message_tool_result_ids)
        .collect::<HashSet<_>>();

    if !required_tool_use_ids.is_empty() {
        for index in (window_start..keep_start).rev() {
            if message_has_tool_use_with_ids(&messages[index], &required_tool_use_ids) {
                required_keep_start = required_keep_start.min(index);
            }
        }
    }

    for kept_message in kept_messages {
        if !matches!(kept_message.role, MessageRole::Assistant) {
            continue;
        }

        let Some(message_id) = kept_message.message_id.as_deref() else {
            continue;
        };

        for index in (window_start..keep_start).rev() {
            let candidate = &messages[index];
            if matches!(candidate.role, MessageRole::Assistant)
                && candidate.message_id.as_deref() == Some(message_id)
            {
                required_keep_start = required_keep_start.min(index);
            }
        }
    }

    required_keep_start
}

fn message_tool_result_ids(message: &ChatMessage) -> Vec<&str> {
    message
        .blocks
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
        .collect()
}

fn message_has_tool_use_with_ids(message: &ChatMessage, tool_use_ids: &HashSet<&str>) -> bool {
    message.blocks.iter().any(|block| {
        matches!(
            block,
            ContentBlock::ToolUse { call_id, .. } if tool_use_ids.contains(call_id.as_str())
        )
    })
}
