//! Frozen JSON baselines for the wire request types.
//!
//! These tests pin the exact serialized shape of the wire request structs —
//! `runtime_id` rename, `session_id` legacy alias acceptance, default-null
//! optional fields, and the open→turn conversion.  They are the protocol
//! regression guardrail: any serde drift here is a wire-contract change that
//! must be coordinated with the daemon repository.

use super::*;
use std::path::PathBuf;

#[test]
fn runtime_open_request_serializes_runtime_id_and_accepts_legacy_session_id() {
    let request = RuntimeOpenRequest {
        session_id: "runtime-1".to_string(),
        conversation_id: "conv-1".to_string(),
        sender_id: "user-1".to_string(),
        entry: GatewayEntryContext::default(),
        channel: None,
        channel_instance_id: None,
        llm: None,
        workspace: None,
        skills: None,
        client_id: None,
        client_pid: None,
        client_hostname: None,
    };

    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(value["runtime_id"], "runtime-1");
    assert!(value.get("session_id").is_none());
    assert!(value["workspace"].is_null());
    assert!(value["skills"].is_null());

    let old: RuntimeOpenRequest =
        serde_json::from_str(r#"{"runtime_id":"old","conversation_id":"c","sender_id":"u"}"#)
            .expect("old request without bootstrap fields should deserialize");
    assert!(old.workspace.is_none());
    assert!(old.skills.is_none());

    let legacy: RuntimeCloseRequest = serde_json::from_str(r#"{"session_id":"legacy-runtime"}"#)
        .expect("legacy session_id should deserialize");
    assert_eq!(legacy.session_id, "legacy-runtime");
}

#[test]
fn open_bootstrap_paths_are_carried_into_direct_turn_conversion() {
    let request: RuntimeOpenRequest = serde_json::from_str(
        r#"{
            "runtime_id":"runtime-1",
            "conversation_id":"conversation",
            "sender_id":"user",
            "workspace":"/home/cz",
            "skills":["/home/cz/.xiaoo/skills","/opt/company/skills"]
        }"#,
    )
    .expect("bootstrap request");

    let turn = request.into_turn_request("hello".to_string());

    assert_eq!(turn.workspace, Some(PathBuf::from("/home/cz")));
    assert_eq!(
        turn.skills,
        Some(vec![
            PathBuf::from("/home/cz/.xiaoo/skills"),
            PathBuf::from("/opt/company/skills")
        ])
    );
}

#[test]
fn runtime_turn_request_serializes_runtime_id() {
    let request = RuntimeTurnRequest {
        session_id: "runtime-1".to_string(),
        entry: GatewayEntryContext::default(),
        channel: None,
        message_id: None,
        conversation_id: "conv-1".to_string(),
        sender_id: "user-1".to_string(),
        text: "hello".to_string(),
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
        reasoning_effort: ReasoningEffort::default(),
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        client_id: None,
    };

    let value = serde_json::to_value(&request).expect("request should serialize");
    assert_eq!(value["runtime_id"], "runtime-1");
    assert!(value.get("session_id").is_none());
    assert!(value["workspace"].is_null());
    assert!(value["skills"].is_null());
}

#[test]
fn runtime_turn_request_accepts_explicit_empty_skills() {
    let request: RuntimeTurnRequest = serde_json::from_str(
        r#"{
            "runtime_id":"runtime-1",
            "conversation_id":"conversation",
            "sender_id":"user",
            "text":"hello",
            "channel":null,
            "message_id":null,
            "channel_instance_id":null,
            "reply_to_message_id":null,
            "root_message_id":null,
            "mentions":[],
            "skills":[]
        }"#,
    )
    .expect("turn request");

    assert_eq!(request.skills, Some(Vec::new()));
    assert!(request.workspace.is_none());
}

#[test]
fn runtime_turn_request_accepts_minimal_json_omitting_all_optional_fields() {
    // Mirrors `SessionOpenRequest`'s minimal-JSON contract: every `Option<T>`
    // field carries `#[serde(default)]`, so a client may omit every optional
    // key (channel / message_id / channel_instance_id /
    // channel_identity_prompt / reply_to_message_id / root_message_id / llm /
    // workspace / skills / command_context / client_id) and only send the
    // four required keys plus the non-optional `mentions` array.
    let request: RuntimeTurnRequest = serde_json::from_str(
        r#"{"runtime_id":"runtime-1","conversation_id":"c","sender_id":"u","text":"hi","mentions":[]}"#,
    )
    .expect("minimal turn request should deserialize");

    assert_eq!(request.session_id, "runtime-1");
    assert_eq!(request.conversation_id, "c");
    assert_eq!(request.sender_id, "u");
    assert_eq!(request.text, "hi");
    assert!(request.channel.is_none());
    assert!(request.message_id.is_none());
    assert!(request.channel_instance_id.is_none());
    assert!(request.channel_identity_prompt.is_none());
    assert!(request.reply_to_message_id.is_none());
    assert!(request.root_message_id.is_none());
    assert!(request.llm.is_none());
    assert!(request.workspace.is_none());
    assert!(request.skills.is_none());
    assert!(request.command_context.is_none());
    assert!(request.client_id.is_none());
    assert_eq!(request.chain_depth, 0);
}
