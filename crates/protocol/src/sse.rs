//! SSE event model for `/api/v1/runtimes/input`.
//!
//! This module provides the strongly-typed [`RuntimeSseEvent`] model — the
//! client-side protocol surface used to consume the daemon's SSE stream.
//! The daemon serializes this exact type: `apps/serverside` re-exports it
//! through `xiaoo_shared::daemon_protocol::sse` (as `SseStreamEvent`), so
//! there is no separate producer-side model to keep field-by-field in sync —
//! this type is the single source of the wire contract.
//!
//! The one genuine consumer-side mirror lives in the endside TUI:
//! `RemoteSseEvent` in `apps/endside/src/gateway_api/remote.rs`. Any
//! field/event change must be synchronized with that mapping.
//!
//! How the event vocabulary is consumed is up to the client — remote
//! session implementations map `RuntimeSseEvent` to their own
//! application-level event models, keeping the protocol surface decoupled
//! from any specific client type.
//!
//! ## Compatibility discipline
//!
//! - The serde representation of these types is a wire contract; any field
//!   addition/removal must be checked against the daemon's actual output.
//! - Unknown fields are always tolerated (`deny_unknown_fields` is not
//!   used) — the daemon may add fields and old clients must silently
//!   ignore them.
//! - Unknown event types yield `Ok(None)` from [`parse_sse_data`] (forward
//!   compatible) — the daemon may add event types and old clients skip
//!   them instead of erroring.
//! - Field defaults (`#[serde(default)]`) match the daemon's
//!   backward-compat fields such as `args_preview` / `detail` / `actions`
//!   / `turn_count` / `total_tokens` / `stop_reason`.

use agent_types::hook::HookAction;
use agent_types::interaction::InteractionRequest;
use agent_types::llm::ChatMessage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn is_zero(value: &u64) -> bool {
    *value == 0
}

use crate::plan::TodoSnapshotItem;

/// A single event in the SSE stream. `agent_id` distinguishes the main
/// agent lane from subagent lanes.
///
/// Single source of the SSE wire contract: the daemon (`apps/serverside`)
/// serializes this exact type via the `xiaoo_shared::daemon_protocol::sse`
/// re-export, and clients deserialize the same shape.
//
// Fields carry no per-field rustdoc: each variant's doc already explains
// the semantics, and field names mirror the daemon wire contract
// field-by-field (the serde representation is the documentation).
// `missing_docs` is allowed to avoid a wall of redundant field-level
// comments.
#[allow(missing_docs)]
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeSseEvent {
    /// Turn started (mirrors daemon `SseStreamEvent::TurnStart`).
    TurnStart { agent_id: String, turn: u32 },
    /// Assistant text delta. `snapshot` is the current full snapshot (for
    /// renderers that need the complete message).
    TextDelta {
        agent_id: String,
        delta: String,
        snapshot: String,
    },
    /// Thinking text delta. Same snapshot semantics as `TextDelta`.
    ThinkingDelta {
        agent_id: String,
        delta: String,
        snapshot: String,
    },
    /// Tool result event (flattened; mirrors daemon `SseStreamEvent::ToolResult`).
    ToolResult {
        agent_id: String,
        call_id: String,
        tool_name: String,
        output_preview: String,
        is_error: bool,
        #[serde(default)]
        args_preview: String,
    },
    /// Per-call file change delta (precomputed by the daemon; mirrors
    /// `SseStreamEvent::ToolFileChange`).
    ToolFileChange {
        call_id: String,
        file_path: String,
        additions: u32,
        deletions: u32,
    },
    /// Plan panel snapshot (parsed by the daemon from `todo_write` args;
    /// mirrors `SseStreamEvent::PlanUpdate`).
    PlanUpdate {
        title: String,
        items: Vec<TodoSnapshotItem>,
    },
    /// Subagent lane metadata (parsed by the daemon from `spawn_subagent`
    /// args; mirrors `SseStreamEvent::SubagentSpawn`).
    SubagentSpawn {
        agent_id: String,
        parent_agent_id: Option<String>,
        title: String,
        description: String,
        task_goal: String,
    },
    /// Tool lifecycle event (flattened; mirrors daemon `SseStreamEvent::ToolCall`).
    /// Only the `Running` status is forwarded; terminal states arrive via
    /// the subsequent `ToolResult` event.
    ToolCall {
        agent_id: String,
        call_id: String,
        tool_name: String,
        #[serde(default)]
        args_preview: String,
        status: ToolCallStatus,
        #[serde(default)]
        detail: String,
    },
    /// Per-agent loop end marker (mirrors `SseStreamEvent::LoopEnd`).
    LoopEnd {
        agent_id: String,
        #[serde(default)]
        turn_count: u32,
        #[serde(default)]
        total_tokens: usize,
        #[serde(default)]
        stop_reason: String,
    },
    /// Interaction request (approval/question); the client must answer via
    /// POST /runtimes/interaction.
    InteractionRequested { request: InteractionRequest },
    /// Terminal event: full messages for this turn + token stats + hook actions.
    Done {
        reply: String,
        raw_reply: String,
        conversation_id: String,
        /// Serde-renamed to `runtime_id` (aligned with daemon `SseStreamEvent::Done`).
        #[serde(rename = "runtime_id")]
        session_id: String,
        turn_count: u32,
        total_tokens: usize,
        prompt_tokens: u64,
        completion_tokens: u64,
        #[serde(default)]
        #[serde(skip_serializing_if = "is_zero")]
        cached_tokens: u64,
        estimated_input_tokens: u64,
        messages: Vec<ChatMessage>,
        stop_reason: String,
        #[serde(default)]
        actions: Vec<HookAction>,
    },
    /// Error termination.
    Error { error: String },
    /// Cancellation event (serde renames `session_id` → `runtime_id` on
    /// the wire).
    Cancelled {
        #[serde(rename = "runtime_id")]
        session_id: String,
    },
    /// Unknown event type (forward compatible). When the daemon adds new
    /// event types, old clients receive `Ok(None)` from [`parse_sse_data`]
    /// instead of an error.
    #[schemars(skip)]
    #[serde(other)]
    Unknown,
}

/// Wire-format mirror of `agent_types::tool::ToolExecutionStatus`. The
/// daemon re-exports this exact type through
/// `xiaoo_shared::daemon_protocol::sse`, so there is no separate
/// serializer-side copy to align with.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    /// Tool args received, executor about to run.
    Running,
    /// Executor returned successfully.
    Completed,
    /// Executor returned an error.
    Failed,
    /// Tool args were rejected by the policy layer before execution.
    Denied,
}

impl RuntimeSseEvent {
    /// Return the stable SSE event name associated with this wire event.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::TurnStart { .. } => "turn_start",
            Self::TextDelta { .. } => "text_delta",
            Self::ThinkingDelta { .. } => "thinking_delta",
            Self::ToolResult { .. } => "tool_result",
            Self::ToolFileChange { .. } => "tool_file_change",
            Self::PlanUpdate { .. } => "plan_update",
            Self::SubagentSpawn { .. } => "subagent_spawn",
            Self::ToolCall { .. } => "tool_call",
            Self::LoopEnd { .. } => "loop_end",
            Self::InteractionRequested { .. } => "interaction_requested",
            Self::Done { .. } => "done",
            Self::Error { .. } => "error",
            Self::Cancelled { .. } => "cancelled",
            Self::Unknown => "unknown",
        }
    }
}

/// Parse a single SSE `data:` payload.
///
/// - Known event type → `Ok(Some(event))`
/// - Unknown event type → `Ok(None)` (forward compatible; old clients skip
///   types the daemon adds later)
/// - Empty/null payload → `Ok(None)`
/// - JSON syntax or field type error → `Err(serde_json::Error)`
pub fn parse_sse_data(data: &str) -> Result<Option<RuntimeSseEvent>, serde_json::Error> {
    let trimmed = data.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(None);
    }
    // Empty strings and literal `null` are short-circuited above to avoid the
    // double parse cost of "Value DOM → typed". Unknown event types land on
    // `Unknown` via `#[serde(other)]` and map to None.
    let event: RuntimeSseEvent = serde_json::from_str(trimmed)?;
    match event {
        RuntimeSseEvent::Unknown => Ok(None),
        other => Ok(Some(other)),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/protocol/sse_test.rs"]
mod tests;
