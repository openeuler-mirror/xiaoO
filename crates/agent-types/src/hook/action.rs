use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Actions a plugin can request alongside its primary hook result.
///
/// Plugins return these by adding an `actions` array to the JSON response:
///
/// ```json
/// {
///   "result": "ack",
///   "actions": [
///     {"kind": "create_session", "session_id": "debug-1"},
///     {"kind": "switch_session", "session_id": "debug-1"}
///   ]
/// }
/// ```
///
/// The `result` field is parsed by the existing adaptor logic (non-breaking).
/// The `actions` field is parsed by [`parse_actions`] and dispatched by the
/// host (daemon/TUI) after the primary hook result is applied. Actions are
/// best-effort: failures are logged and skipped, never propagated to the
/// caller of the hook.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HookAction {
    /// Create (or resume) a session with the given id on the daemon side.
    /// Daemon calls `open_session` (idempotent resume). The action is
    /// forwarded to the TUI so it can switch focus.
    CreateSession { session_id: String },
    /// Switch the TUI focus to an existing session. Daemon proactively calls
    /// `open_session` to ensure the target exists (idempotent resume), then
    /// forwards the action to the TUI.
    SwitchSession { session_id: String },
    /// Forward a prompt to the target session. Daemon proactively calls
    /// `open_session` to ensure the target exists, then forwards the action
    /// to the TUI. The TUI switches focus (if not already on the target),
    /// echoes the prompt locally, and starts a turn via
    /// `POST /api/v1/runtimes/input`. Remote-mode only; in local mode the
    /// action is dropped by the TUI.
    ///
    /// `chain_depth` is **host-controlled**: plugins must not set it. The
    /// daemon stamps it (`emitting turn depth + 1`) before forwarding so the
    /// cross-turn depth cap can be enforced; the TUI relays the stamped
    /// value back via `RuntimeTurnRequest.chain_depth` so the resulting
    /// turn's depth is tracked. A normal user-typed turn carries
    /// `chain_depth = 0`, which resets the chain.
    SendPrompt {
        session_id: String,
        text: String,
        #[serde(default)]
        chain_depth: usize,
    },
}

/// Extract the `actions` array from a plugin's JSON response.
///
/// Returns an empty vec when the field is missing, not an array, or contains
/// entries that fail to deserialize as [`HookAction`]. Invalid entries are
/// silently skipped so a single malformed action does not poison the rest.
pub fn parse_actions(output: &serde_json::Value) -> Vec<HookAction> {
    output
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| serde_json::from_value(v.clone()).ok())
        .collect()
}

#[cfg(test)]
#[path = "../../../../tests/unit/agent-types/hook/action_test.rs"]
mod tests;
