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
/// transitions. `state` carries the new session-lifecycle state tag (e.g.
/// `"idle"` = session back to idle, ready for the next turn); `outcome`
/// carries the turn's terminal kind (`"complete"` / `"max_turns_reached"` /
/// `"budget_exhausted"` / `"cancelled"`) so plugins can distinguish a normal
/// completion from a soft termination while still seeing the same
/// `state="idle"`. Currently dispatched only after a non-error root turn
/// termination (any `Ok` variant of `AgentOutcome`); the `String` types let
/// future call sites emit other tags (`"running"`, `"failed"`, ...) without
/// changing this contract. Dispatched fire-and-forget so plugin scripts
/// cannot block the turn result.
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
