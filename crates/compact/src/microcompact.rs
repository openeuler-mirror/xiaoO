use std::collections::{HashMap, HashSet};

use agent_contracts::TokenEstimator;
use agent_types::compression::MicroCompactResult;
use agent_types::{ChatMessage, ContentBlock};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MicroCompactionPolicy {
    pub stale_tool_pair_after_ms: u64,
    pub preserve_recent_messages: usize,
}

#[derive(Default)]
struct ToolPairWindow {
    assistant_index: Option<usize>,
    tool_index: Option<usize>,
    latest_timestamp: u64,
}

pub fn apply_microcompact(
    messages: &[ChatMessage],
    now_ms: u64,
    estimator: &dyn TokenEstimator,
    policy: &MicroCompactionPolicy,
) -> MicroCompactResult {
    let protected_tail_start = messages
        .len()
        .saturating_sub(policy.preserve_recent_messages);
    let mut windows = HashMap::<String, ToolPairWindow>::new();

    for (index, message) in messages.iter().enumerate() {
        for block in &message.blocks {
            match block {
                ContentBlock::ToolUse { call_id, .. } => {
                    let window = windows.entry(call_id.clone()).or_default();
                    window.assistant_index = Some(index);
                    window.latest_timestamp = window.latest_timestamp.max(message.timestamp_ms);
                }
                ContentBlock::ToolResult { call_id, .. } => {
                    let window = windows.entry(call_id.clone()).or_default();
                    window.tool_index = Some(index);
                    window.latest_timestamp = window.latest_timestamp.max(message.timestamp_ms);
                }
                ContentBlock::Text { .. }
                | ContentBlock::Image { .. }
                | ContentBlock::Document { .. } => {}
            }
        }
    }

    let mut removable_call_ids = windows
        .into_iter()
        .filter_map(|(call_id, window)| {
            let assistant_index = window.assistant_index?;
            let tool_index = window.tool_index?;
            let latest_index = assistant_index.max(tool_index);
            let age_ms = now_ms.saturating_sub(window.latest_timestamp);
            if latest_index >= protected_tail_start || age_ms < policy.stale_tool_pair_after_ms {
                return None;
            }

            Some(call_id)
        })
        .collect::<Vec<_>>();
    removable_call_ids.sort();

    let removable_lookup = removable_call_ids
        .iter()
        .cloned()
        .collect::<HashSet<String>>();

    let estimated_before = estimator.estimate_messages_tokens(messages);
    let filtered_messages = messages
        .iter()
        .filter(|message| !is_removable_tool_message(message, &removable_lookup))
        .cloned()
        .collect::<Vec<_>>();
    let estimated_after = estimator.estimate_messages_tokens(&filtered_messages);

    MicroCompactResult {
        applied: filtered_messages.len() != messages.len(),
        removed_count: messages.len() - filtered_messages.len(),
        removed_call_ids: removable_call_ids,
        messages: filtered_messages,
        token_delta: estimated_before as isize - estimated_after as isize,
    }
}

fn is_removable_tool_message(message: &ChatMessage, removable_lookup: &HashSet<String>) -> bool {
    let mut saw_tool_block = false;

    for block in &message.blocks {
        match block {
            ContentBlock::ToolUse { call_id, .. } | ContentBlock::ToolResult { call_id, .. } => {
                saw_tool_block = true;
                if !removable_lookup.contains(call_id) {
                    return false;
                }
            }
            ContentBlock::Text { .. }
            | ContentBlock::Image { .. }
            | ContentBlock::Document { .. } => return false,
        }
    }

    saw_tool_block
}
