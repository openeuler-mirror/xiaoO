//! Turn request/result types for the gateway service layer.
//!
//! The wire-facing request types ([`AppTurnRequest`] and its supporting
//! [`GatewayEntryContext`] / [`GatewayEntryKind`] / [`LlmRuntimeConfig`] /
//! [`TurnMention`]) live in [`protocol::wire`] and are re-exported here
//! to preserve existing import paths. The service-layer result type
//! [`AppTurnResult`] and its [`TurnOutcome`] tag stay defined here — they are
//! in-process turn outcomes, not wire-contract payloads, so they belong in
//! the service layer.

pub use protocol::wire::{
    AppTurnRequest, GatewayEntryContext, GatewayEntryKind, LlmRuntimeConfig, RuntimeTurnRequest,
    TurnMention,
};

use agent_types::ChatMessage;
use serde::{Deserialize, Serialize};

/// Terminal kind of a completed (non-error) root turn, mirroring the four
/// `Ok` variants of [`agent_types::outcome::AgentOutcome`]. Carried on
/// [`AppTurnResult`] so downstream consumers (notably the
/// `*.Session.lifecycle.state` plugin hook) can distinguish a normal
/// completion from a soft termination (`max_turns_reached` /
/// `budget_exhausted` / `cancelled`). True errors stay on the `Err` side of
/// `Result<AppTurnResult, _>` and are out of scope for this enum.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnOutcome {
    Complete,
    MaxTurnsReached,
    BudgetExhausted,
    Cancelled,
}

impl TurnOutcome {
    /// Snake-case tag used in hook payloads (matches the serde renaming).
    pub fn as_tag(&self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::MaxTurnsReached => "max_turns_reached",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppTurnResult {
    pub raw_reply: String,
    pub visible_reply: String,
    pub messages: Vec<ChatMessage>,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub estimated_input_tokens: u64,
    pub outcome: TurnOutcome,
    /// Actions requested by `*.Session.lifecycle.state` plugin hookers after
    /// the turn terminated. Collected by `run_turn_inner` (await variant, not
    /// fire-and-forget) and executed daemon-side via `HookActionSink`. Callers
    /// that expose a UI (daemon HTTP/SSE, TUI) forward these to the client so
    /// it can switch session focus / sync titles. CLI and channel callers
    /// currently ignore this field.
    pub hook_actions: Vec<agent_types::hook::HookAction>,
}
