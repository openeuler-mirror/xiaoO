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

    // Tool-call protocol messages are atomic: an assistant message may contain
    // several calls, and the provider requires a result for *every* call in that
    // message.  Only compact a whole assistant call message when every call in
    // it has a stale, paired result outside the protected tail.  In particular,
    // never remove one result from a multi-call assistant message while keeping
    // the assistant message (which would leave a dangling tool_call_id).
    let mut removable_call_ids = HashSet::<String>::new();
    for (assistant_index, message) in messages.iter().enumerate() {
        if assistant_index >= protected_tail_start {
            continue;
        }
        let call_ids = message
            .blocks
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolUse { call_id, .. } => Some(call_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        if call_ids.is_empty()
            || message
                .blocks
                .iter()
                .any(|block| !matches!(block, ContentBlock::ToolUse { .. }))
        {
            continue;
        }

        let all_stale_and_paired = call_ids.iter().all(|call_id| {
            let Some(window) = windows.get(call_id) else {
                return false;
            };
            if window.assistant_index != Some(assistant_index) {
                return false;
            }
            let Some(tool_index) = window.tool_index else {
                return false;
            };
            tool_index < protected_tail_start
                && now_ms.saturating_sub(window.latest_timestamp) >= policy.stale_tool_pair_after_ms
        });
        if !all_stale_and_paired {
            continue;
        }

        // A result message can itself contain multiple blocks.  Keep the
        // pairing atomic there too: if it contains an unqualified result, do
        // not remove any call from this assistant message.
        let result_messages_are_atomic = call_ids.iter().all(|call_id| {
            let tool_index = windows
                .get(call_id)
                .and_then(|window| window.tool_index)
                .expect("paired tool result checked above");
            messages[tool_index].blocks.iter().all(|block| match block {
                ContentBlock::ToolResult { call_id: id, .. } => call_ids.contains(id),
                _ => false,
            })
        });
        if result_messages_are_atomic {
            removable_call_ids.extend(call_ids);
        }
    }
    let mut removable_call_ids = removable_call_ids.into_iter().collect::<Vec<_>>();
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
