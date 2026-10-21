use super::*;

use super::super::super::test_support::{self, block_on, TestRuntimeView};

use agent_types::chat::ModelRef;
use agent_types::common::HookerId;
use agent_types::hook::{HookInvokePrimary, HookPointId};
use agent_types::{ContentBlock, MessageRole};

fn user_text(text: &str) -> ChatMessage {
    ChatMessage {
        role: MessageRole::User,
        blocks: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        message_id: None,
        timestamp_ms: 0,
        api_usage_tokens: None,
        reasoning_content: None,
        estimated_tokens: None,
    }
}

// ---- minimal test runtime view --------------------------------------
// The shared scaffold (TestRuntimeView and friends) lives in
// `test_support.rs`, mounted at `crates/hook/src/hookers/plugin/mod.rs`.

fn adaptor_for(command: &str) -> PluginChatHookerAdaptor {
    PluginChatHookerAdaptor::new(
        HookerId("plugin_chat_test".to_string()),
        HookPointId("test-agent.Chat.system.transform".to_string()),
        command.to_string(),
        Value::Null,
    )
}

/// Write `json` to a temp file and return a `sh -c` command string that
/// prints it (see [`test_support::cat_command_for`]).
fn cat_command_for(json: &str) -> String {
    test_support::cat_command_for("xiaoo_chat_hook_test", json)
}

// ---- system transform ----------------------------------------------

#[test]
fn system_transform_allow_keeps_parts() {
    let adaptor = adaptor_for(&cat_command_for(r#"{"result":"allow"}"#));
    let runtime = TestRuntimeView::new();
    let input = ChatSystemTransformInput {
        session_id: Some("s1".to_string()),
        model: ModelRef::default(),
        current_system: vec!["base".to_string()],
    };
    let output =
        block_on(adaptor.invoke_system_transform(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatSystemTransform(ChatSystemTransformResult::Allow) => {}
        other => panic!("expected Allow, got {:?}", other),
    }
}

#[test]
fn system_transform_transform_replaces_parts() {
    let adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"transform","system":["new","parts"]}"#,
    ));
    let runtime = TestRuntimeView::new();
    let input = ChatSystemTransformInput {
        session_id: Some("s1".to_string()),
        model: ModelRef::default(),
        current_system: vec!["base".to_string()],
    };
    let output =
        block_on(adaptor.invoke_system_transform(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatSystemTransform(ChatSystemTransformResult::Transform { system }) => {
            assert_eq!(system, vec!["new".to_string(), "parts".to_string()]);
        }
        other => panic!("expected Transform, got {:?}", other),
    }
}

// ---- chat message ---------------------------------------------------

#[test]
fn chat_message_accept_keeps_message() {
    let mut adaptor = adaptor_for(&cat_command_for(r#"{"result":"accept"}"#));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
    let runtime = TestRuntimeView::new();
    let candidate = user_text("hello");
    let input = ChatMessageHookInput {
        session_id: "s1".to_string(),
        agent: Some("test-agent".to_string()),
        model: None,
        message_id: None,
        message: candidate,
        prior_message_count: 0,
    };
    let output =
        block_on(adaptor.invoke_chat_message(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatMessage(ChatMessageHookResult::Accept) => {}
        other => panic!("expected Accept, got {:?}", other),
    }
}

#[test]
fn chat_message_transform_replaces_message() {
    let mut adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"transform","message":{"role":"user","blocks":[{"type":"text","text":"redacted"}],"timestamp_ms":0,"message_id":null,"api_usage_tokens":null,"reasoning_content":null,"estimated_tokens":null}}"#,
    ));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
    let runtime = TestRuntimeView::new();
    let candidate = user_text("secret sk-abc");
    let input = ChatMessageHookInput {
        session_id: "s1".to_string(),
        agent: Some("test-agent".to_string()),
        model: None,
        message_id: None,
        message: candidate,
        prior_message_count: 0,
    };
    let output =
        block_on(adaptor.invoke_chat_message(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::ChatMessage(ChatMessageHookResult::Transform { message }) => {
            assert_eq!(message.blocks.len(), 1);
            match &message.blocks[0] {
                ContentBlock::Text { text } => assert_eq!(text, "redacted"),
                other => panic!("expected text block, got {:?}", other),
            }
        }
        other => panic!("expected Transform, got {:?}", other),
    }
}

// ---- command before -------------------------------------------------

#[test]
fn command_before_allow_keeps_body() {
    let mut adaptor = adaptor_for(&cat_command_for(r#"{"result":"allow"}"#));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let runtime = TestRuntimeView::new();
    let input = CommandExecuteBeforeInput {
        command: "review".to_string(),
        session_id: "s1".to_string(),
        arguments: "src/main.rs".to_string(),
        body: "Review this carefully.\n\nsrc/main.rs".to_string(),
    };
    let output =
        block_on(adaptor.invoke_command_before(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::CommandExecuteBefore(CommandExecuteBeforeResult::Allow) => {}
        other => panic!("expected Allow, got {:?}", other),
    }
}

#[test]
fn command_before_transform_rewrites_body() {
    let mut adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"transform","body":"rewritten body"}"#,
    ));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let runtime = TestRuntimeView::new();
    let input = CommandExecuteBeforeInput {
        command: "review".to_string(),
        session_id: "s1".to_string(),
        arguments: "".to_string(),
        body: "original".to_string(),
    };
    let output =
        block_on(adaptor.invoke_command_before(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::CommandExecuteBefore(CommandExecuteBeforeResult::Transform { body }) => {
            assert_eq!(body, "rewritten body");
        }
        other => panic!("expected Transform, got {:?}", other),
    }
}

#[test]
fn command_before_deny_carries_reason() {
    let mut adaptor = adaptor_for(&cat_command_for(
        r#"{"result":"deny","reason":"blocked by policy"}"#,
    ));
    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let runtime = TestRuntimeView::new();
    let input = CommandExecuteBeforeInput {
        command: "deploy".to_string(),
        session_id: "s1".to_string(),
        arguments: "".to_string(),
        body: "deploy prod".to_string(),
    };
    let output =
        block_on(adaptor.invoke_command_before(&input, &HookInvokeMetadata::default(), &runtime))
            .unwrap();
    match output.primary {
        HookInvokePrimary::CommandExecuteBefore(CommandExecuteBeforeResult::Deny { reason }) => {
            assert_eq!(reason, "blocked by policy");
        }
        other => panic!("expected Deny, got {:?}", other),
    }
}

// ---- payload workspace field ------------------------------------------

#[test]
fn chat_payloads_carry_workspace_root() {
    let runtime = TestRuntimeView::new();

    let mut adaptor = adaptor_for("cat > /dev/null");

    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.command.before".to_string()));
    let command_input = CommandExecuteBeforeInput {
        command: "review".to_string(),
        session_id: "s1".to_string(),
        arguments: "".to_string(),
        body: "body".to_string(),
    };
    let payload = adaptor.build_command_before_payload(
        &command_input,
        &HookInvokeMetadata::default(),
        &runtime,
    );
    assert_eq!(payload["stage"], json!("command_before"));
    assert_eq!(payload["workspace"], json!("/tmp"));

    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.message.received".to_string()));
    let message_input = ChatMessageHookInput {
        session_id: "s1".to_string(),
        agent: None,
        model: None,
        message_id: None,
        message: user_text("hello"),
        prior_message_count: 0,
    };
    let payload = adaptor.build_chat_message_payload(
        &message_input,
        &HookInvokeMetadata::default(),
        &runtime,
    );
    assert_eq!(payload["stage"], json!("chat_message"));
    assert_eq!(payload["workspace"], json!("/tmp"));

    adaptor
        .core
        .set_hook_point(HookPointId("test-agent.Chat.system.transform".to_string()));
    let system_input = ChatSystemTransformInput {
        session_id: Some("s1".to_string()),
        model: ModelRef::default(),
        current_system: vec!["base".to_string()],
    };
    let payload = adaptor.build_system_transform_payload(
        &system_input,
        &HookInvokeMetadata::default(),
        &runtime,
    );
    assert_eq!(payload["stage"], json!("system_transform"));
    assert_eq!(payload["workspace"], json!("/tmp"));
}
