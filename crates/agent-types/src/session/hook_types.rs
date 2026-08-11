#[derive(Clone, Debug)]
pub struct SessionCreatedHookInput {
    pub session_id: String,
    pub sender_id: String,
}

#[derive(Clone, Debug)]
pub struct SessionClosedHookInput {
    pub session_id: String,
    pub sender_id: String,
}

/// Input for `*.Session.lifecycle.state`. Fires on session lifecycle state
/// transitions. `state` carries the new session-lifecycle state tag:
/// - `"idle"` — session back to idle after a non-error root turn
///   termination, ready for the next turn (awaited; actions collected);
/// - `"failed"` — turn terminated with an `Err` (fire-and-forget; outcome
///   is `"error"`).
///
/// `outcome` carries the turn's terminal kind so plugins can distinguish
/// sub-variants while still seeing the same `state`. For `state="idle"` it
/// is one of `"complete"` / `"max_turns_reached"` / `"budget_exhausted"` /
/// `"cancelled"` (the four `Ok` variants of `AgentOutcome`); for
/// `state="failed"` it is `"error"`. Currently dispatched only at the
/// root-turn boundary in the gateway layer
/// (`CoreBackedSessionService::run_turn_inner`). The `String` types let
/// future call sites emit other tags without changing this contract.
#[derive(Clone, Debug)]
pub struct SessionStateHookInput {
    pub session_id: String,
    pub sender_id: String,
    pub agent_id: String,
    pub state: String,
    pub outcome: String,
}

#[derive(Clone, Debug)]
pub enum SessionHookResult {
    Acknowledged,
}

/// Error type for session-level plugin hookers. Mirrors [`ChatHookError`]:
/// the only failure mode is a plugin command/IO/JSON contract violation,
/// surfaced as a human-readable message to logs and trace spans. Distinct
/// from `ToolExecutionError` so `HookInvokeError` can label session-hook
/// failures via its own variant rather than misfiling them as tool errors.
///
/// [`ChatHookError`]: crate::chat::ChatHookError
#[derive(Debug, thiserror::Error)]
pub enum SessionHookError {
    #[error("{message}")]
    Plugin { message: String },
}
