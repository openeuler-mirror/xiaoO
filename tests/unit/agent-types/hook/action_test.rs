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
