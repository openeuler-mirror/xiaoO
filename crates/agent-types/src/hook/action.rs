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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
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
mod tests {
    use super::{parse_actions, HookAction};
    use serde_json::json;

    #[test]
    fn parse_actions_empty_when_field_missing() {
        let parsed = parse_actions(&json!({"result": "ack"}));
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_actions_collects_all_valid_entries() {
        let parsed = parse_actions(&json!({
            "result": "ack",
            "actions": [
                {"kind": "create_session", "session_id": "a"},
                {"kind": "switch_session", "session_id": "a"}
            ]
        }));
        assert_eq!(parsed.len(), 2);
        assert!(matches!(parsed[0], HookAction::CreateSession { .. }));
        assert!(matches!(parsed[1], HookAction::SwitchSession { .. }));
    }

    #[test]
    fn parse_actions_skips_invalid_entries() {
        let parsed = parse_actions(&json!({
            "actions": [
                {"kind": "switch_session", "session_id": "ok"},
                {"kind": "unknown_kind", "session_id": "bad"},
                "not-an-object"
            ]
        }));
        assert_eq!(parsed.len(), 1);
        assert!(matches!(parsed[0], HookAction::SwitchSession { .. }));
    }

    #[test]
    fn parse_actions_returns_empty_when_not_array() {
        let parsed = parse_actions(&json!({"actions": "not-an-array"}));
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_actions_collects_send_prompt() {
        let parsed = parse_actions(&json!({
            "result": "ack",
            "actions": [
                {"kind": "create_session", "session_id": "a"},
                {"kind": "switch_session", "session_id": "a"},
                {"kind": "send_prompt", "session_id": "a", "text": "hello"}
            ]
        }));
        assert_eq!(parsed.len(), 3);
        assert!(matches!(parsed[0], HookAction::CreateSession { .. }));
        assert!(matches!(parsed[1], HookAction::SwitchSession { .. }));
        match &parsed[2] {
            HookAction::SendPrompt {
                session_id,
                text,
                chain_depth,
            } => {
                assert_eq!(session_id, "a");
                assert_eq!(text, "hello");
                assert_eq!(*chain_depth, 0, "plugin-omitted chain_depth defaults to 0");
            }
            _ => panic!("expected SendPrompt"),
        }
    }

    #[test]
    fn parse_actions_preserves_plugin_supplied_chain_depth() {
        // Plugins normally omit chain_depth; if one sets it, the value is
        // preserved through parsing (the daemon overwrites it before
        // forwarding, so plugin-supplied values cannot bypass the cap).
        let parsed = parse_actions(&json!({
            "actions": [
                {"kind": "send_prompt", "session_id": "a", "text": "x", "chain_depth": 7}
            ]
        }));
        match &parsed[0] {
            HookAction::SendPrompt { chain_depth, .. } => assert_eq!(*chain_depth, 7),
            _ => panic!("expected SendPrompt"),
        }
    }
}
