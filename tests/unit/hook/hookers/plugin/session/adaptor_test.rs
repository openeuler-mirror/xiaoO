use super::*;

use super::super::super::test_support::{self, block_on, TestRuntimeView};

use agent_types::common::HookerId;
use agent_types::hook::{HookInvokePrimary, HookPointId};

fn adaptor_for(command: &str) -> PluginSessionHookerAdaptor {
    PluginSessionHookerAdaptor::new(
        HookerId("plugin_session_test".to_string()),
        HookPointId("defaultagent.Session.lifecycle.state".to_string()),
        command.to_string(),
        Value::Null,
    )
}

fn idle_input() -> SessionStateHookInput {
    SessionStateHookInput {
        session_id: "s1".to_string(),
        sender_id: "u1".to_string(),
        agent_id: "defaultagent".to_string(),
        state: "idle".to_string(),
        outcome: "complete".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    }
}

/// Write `json` to a temp file and return a `sh -c` command string that
/// prints it (see [`test_support::cat_command_for`]).
fn cat_command_for(json: &str) -> String {
    test_support::cat_command_for("xiaoo_session_hook_test", json)
}

#[test]
fn session_state_ack_returns_acknowledged() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"ack"}"#));
    let runtime = TestRuntimeView::new();
    let output = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ))
    .unwrap();
    match output.primary {
        HookInvokePrimary::SessionState(SessionHookResult::Acknowledged) => {}
        other => panic!("expected SessionState(Acknowledged), got {:?}", other),
    }
}

#[test]
fn session_state_acknowledged_alias_also_accepted() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"acknowledged"}"#));
    let runtime = TestRuntimeView::new();
    let output = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ))
    .unwrap();
    match output.primary {
        HookInvokePrimary::SessionState(SessionHookResult::Acknowledged) => {}
        other => panic!("expected SessionState(Acknowledged), got {:?}", other),
    }
}

#[test]
fn session_state_transform_result_is_rejected() {
    // Event-style hooks have no mutable output; transform is invalid.
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"transform","system":["x"]}"#));
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(
        result.is_err(),
        "transform should be rejected for state hook"
    );
}

#[test]
fn session_state_missing_result_field_errors() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"foo":"bar"}"#));
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_state_nonzero_exit_errors() {
    // `false` exits with status 1.
    let adaptor = adaptor_for("cat > /dev/null; false");
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_state_invalid_json_errors() {
    let adaptor = adaptor_for("cat > /dev/null; echo not-json");
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_state(
        &idle_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_state_payload_carries_state_and_session_fields() {
    // Verify the payload JSON contains the expected fields before the
    // command ignores them. Use a script that echoes the payload back so
    // we can inspect it.
    let adaptor = adaptor_for("cat");
    let runtime = TestRuntimeView::new();
    let input = SessionStateHookInput {
        session_id: "s-idle-1".to_string(),
        sender_id: "u-sender-1".to_string(),
        agent_id: "agent-x".to_string(),
        state: "idle".to_string(),
        outcome: "complete".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    };
    let payload =
        adaptor.build_session_state_payload(&input, &HookInvokeMetadata::default(), &runtime);
    assert_eq!(payload["stage"], json!("session_state"));
    assert_eq!(payload["state"], json!("idle"));
    assert_eq!(payload["outcome"], json!("complete"));
    assert_eq!(payload["session_id"], json!("s-idle-1"));
    assert_eq!(payload["sender_id"], json!("u-sender-1"));
    assert_eq!(payload["agent_id"], json!("agent-x"));
    assert_eq!(payload["workspace"], json!("/tmp/xiaoo-ws"));
    assert_eq!(payload["hooker"]["id"], json!("plugin_session_test"));
    assert_eq!(
        payload["hooker"]["hook_point"],
        json!("defaultagent.Session.lifecycle.state")
    );
}

#[test]
fn session_state_payload_workspace_none_serializes_as_null() {
    let adaptor = adaptor_for("cat");
    let runtime = TestRuntimeView::new();
    let mut input = idle_input();
    input.workspace = None;
    let payload =
        adaptor.build_session_state_payload(&input, &HookInvokeMetadata::default(), &runtime);
    assert_eq!(payload["workspace"], Value::Null);
}

// ---- session created -------------------------------------------------

fn created_input() -> SessionCreatedHookInput {
    SessionCreatedHookInput {
        session_id: "s1".to_string(),
        sender_id: "u1".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    }
}

#[test]
fn session_created_ack_returns_acknowledged() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"ack"}"#));
    let runtime = TestRuntimeView::new();
    let output = block_on(adaptor.invoke_session_created(
        &created_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ))
    .unwrap();
    match output.primary {
        HookInvokePrimary::SessionCreated(SessionHookResult::Acknowledged) => {}
        other => panic!("expected SessionCreated(Acknowledged), got {:?}", other),
    }
}

#[test]
fn session_created_transform_result_is_rejected() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"transform"}"#));
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_created(
        &created_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(
        result.is_err(),
        "transform should be rejected for created hook"
    );
}

#[test]
fn session_created_payload_carries_identity_fields() {
    let adaptor = adaptor_for("cat");
    let runtime = TestRuntimeView::new();
    let input = SessionCreatedHookInput {
        session_id: "s-created-1".to_string(),
        sender_id: "u-sender-1".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    };
    let payload =
        adaptor.build_session_created_payload(&input, &HookInvokeMetadata::default(), &runtime);
    assert_eq!(payload["stage"], json!("session_created"));
    assert_eq!(payload["session_id"], json!("s-created-1"));
    assert_eq!(payload["sender_id"], json!("u-sender-1"));
    assert_eq!(payload["workspace"], json!("/tmp/xiaoo-ws"));
    assert_eq!(payload["hooker"]["id"], json!("plugin_session_test"));
    assert_eq!(payload["policy"], Value::Null);
    assert_eq!(payload["definition"], Value::Null);
}

// ---- session closed --------------------------------------------------

fn closed_input() -> SessionClosedHookInput {
    SessionClosedHookInput {
        session_id: "s1".to_string(),
        sender_id: "u1".to_string(),
        workspace: Some("/tmp/xiaoo-ws".to_string()),
    }
}

#[test]
fn session_closed_ack_returns_acknowledged() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"ack"}"#));
    let runtime = TestRuntimeView::new();
    let output = block_on(adaptor.invoke_session_closed(
        &closed_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ))
    .unwrap();
    match output.primary {
        HookInvokePrimary::SessionClosed(SessionHookResult::Acknowledged) => {}
        other => panic!("expected SessionClosed(Acknowledged), got {:?}", other),
    }
}

#[test]
fn session_closed_missing_result_field_errors() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"foo":"bar"}"#));
    let runtime = TestRuntimeView::new();
    let result = block_on(adaptor.invoke_session_closed(
        &closed_input(),
        &HookInvokeMetadata::default(),
        &runtime,
    ));
    assert!(result.is_err());
}

#[test]
fn session_closed_payload_carries_identity_fields() {
    let adaptor = adaptor_for("cat");
    let runtime = TestRuntimeView::new();
    let input = SessionClosedHookInput {
        session_id: "s-closed-1".to_string(),
        sender_id: "u-sender-1".to_string(),
        workspace: None,
    };
    let payload =
        adaptor.build_session_closed_payload(&input, &HookInvokeMetadata::default(), &runtime);
    assert_eq!(payload["stage"], json!("session_closed"));
    assert_eq!(payload["session_id"], json!("s-closed-1"));
    assert_eq!(payload["sender_id"], json!("u-sender-1"));
    assert_eq!(payload["workspace"], Value::Null);
    assert_eq!(payload["hooker"]["id"], json!("plugin_session_test"));
}
