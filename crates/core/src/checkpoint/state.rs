use super::Snapshot;
use crate::{LoopState, LoopStateSnapshot};
use agent_types::{ChatMessage, ContentBlock, MessageRole};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredCheckpoint {
    pub snapshot: Snapshot,
    pub context: Arc<ContextState>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ContextState {
    loop_state: LoopStateSnapshot,
    /// A completed assistant response at the tail marks an idle snapshot.
    ends_with_response: bool,
    turn_number: u32,
    plan_nudged: bool,
    last_failure_sig: Option<u64>,
    repeated_failure_count: u32,
    last_success_sig: Option<u64>,
    repeated_success_count: u32,
    tool_executed: bool,
    todos: Vec<tool::CheckpointTodoItem>,
}
impl ContextState {
    pub fn capture(state: &LoopState, turn_number: u32) -> Self {
        let loop_state = state.to_snapshot();
        let ends_with_response = loop_state.messages.last().is_some_and(|message| {
            message.role == MessageRole::Assistant
                && !message
                    .blocks
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolUse { .. }))
        });
        Self {
            loop_state,
            ends_with_response,
            turn_number,
            plan_nudged: state.plan_nudged,
            last_failure_sig: state.last_failure_sig,
            repeated_failure_count: state.repeated_failure_count,
            last_success_sig: state.last_success_sig,
            repeated_success_count: state.repeated_success_count,
            tool_executed: state.tool_executed,
            todos: tool::checkpoint_todos(&state.session_id),
        }
    }
    pub fn restore(
        &self,
        state: &mut LoopState,
        turn_number: &mut u32,
        continuing: bool,
        new_input: Option<&ChatMessage>,
    ) {
        self.apply(state, turn_number);
        if self.ends_with_response && continuing {
            let mut messages = state.messages.write();
            if let Some(input) = new_input {
                // Idle A + the new input that starts this invocation.
                messages.push(input.clone());
            } else {
                // Continue within a running loop by replaying the final step.
                messages.pop();
            }
        }
    }

    pub(super) fn todos(&self) -> &[tool::CheckpointTodoItem] {
        &self.todos
    }

    pub fn apply(&self, state: &mut LoopState, turn_number: &mut u32) {
        // Keep the Arc shared with RuntimeView, and the current cancellation token.
        *state.messages.write() = self.loop_state.messages.clone();
        state.turn_count = self.loop_state.turn_count;
        state.token_usage = self.loop_state.token_usage.clone();
        state.last_token_usage = self.loop_state.last_token_usage.clone();
        state.compression_meta = self.loop_state.compression_meta.clone();
        state.kv_cache_map = self.loop_state.kv_cache_map.clone();
        state.plan_nudged = self.plan_nudged;
        state.last_failure_sig = self.last_failure_sig;
        state.repeated_failure_count = self.repeated_failure_count;
        state.last_success_sig = self.last_success_sig;
        state.repeated_success_count = self.repeated_success_count;
        state.tool_executed = self.tool_executed;
        *turn_number = self.turn_number;
        tool::restore_checkpoint_todos(&state.session_id, self.todos.clone());
    }
}
