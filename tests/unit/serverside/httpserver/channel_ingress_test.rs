use super::{
    build_channel_turn_request, build_gateway_channel_message, GatewayChannelIngressError,
    GatewayChannelMention, GatewayChannelMessage,
};
use crate::channels::{ChannelMention, ChannelMessage};
use xiaoo_shared::channels::ChannelAttachment;

#[test]
fn builds_channel_turn_request_with_instance_scoped_session_id() {
    let message = GatewayChannelMessage {
        channel: "feishu".to_string(),
        channel_instance_id: Some("ops-feishu".to_string()),
        conversation_id: "conv-1".to_string(),
        sender_id: "user-1".to_string(),
        agent_preset_id: Some("code-reviewer".to_string()),
        message_id: "msg-1".to_string(),
        text: "ping".to_string(),
        channel_identity_prompt: Some("<participant_directory />".to_string()),
        reply_to_message_id: Some("prev-1".to_string()),
        root_message_id: Some("root-1".to_string()),
        mentions: vec![GatewayChannelMention {
            id: "bot".to_string(),
            display_name: Some("XiaoO".to_string()),
        }],
    };

    let request = build_channel_turn_request(&message);

    assert_eq!(request.session_id, "ops-feishu:conv-1");
    assert_eq!(request.entry.instance_id.as_deref(), Some("ops-feishu"));
    assert_eq!(
        request.entry.runtime_profile_id.as_deref(),
        Some("code-reviewer")
    );
    assert_eq!(request.channel.as_deref(), Some("feishu"));
    assert_eq!(request.message_id.as_deref(), Some("msg-1"));
    assert_eq!(
        request.client_id.as_deref(),
        Some("daemon:channel:feishu"),
        "channel-ingress turn must carry a daemon channel principal id"
    );
    assert_eq!(
        request.channel_identity_prompt.as_deref(),
        Some("<participant_directory />")
    );
    assert_eq!(request.mentions.len(), 1);
    assert_eq!(request.mentions[0].id, "bot");
}

#[test]
fn falls_back_to_channel_name_when_instance_id_is_absent() {
    let message = GatewayChannelMessage {
        channel: "dingtalk".to_string(),
        channel_instance_id: None,
        conversation_id: "conv-2".to_string(),
        sender_id: "user-2".to_string(),
        agent_preset_id: None,
        message_id: "msg-2".to_string(),
        text: "hello".to_string(),
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
    };

    let request = build_channel_turn_request(&message);

    assert_eq!(request.session_id, "dingtalk:conv-2");
    assert_eq!(request.entry.instance_id, None);
}

#[test]
fn converts_channel_message_without_attachments() {
    let message = ChannelMessage {
        channel: "feishu".to_string(),
        channel_instance_id: Some("ops-feishu".to_string()),
        conversation_id: "conv-3".to_string(),
        sender_id: "user-3".to_string(),
        message_id: "msg-3".to_string(),
        text: "hello".to_string(),
        reply_to_message_id: None,
        root_message_id: None,
        mentions: vec![ChannelMention {
            id: "bot".to_string(),
            display_name: Some("XiaoO".to_string()),
        }],
        attachments: Vec::new(),
    };

    let gateway_message = build_gateway_channel_message(message).expect("message should convert");

    assert_eq!(gateway_message.channel, "feishu");
    assert_eq!(
        gateway_message.channel_instance_id.as_deref(),
        Some("ops-feishu")
    );
    assert!(gateway_message.channel_identity_prompt.is_none());
    assert_eq!(gateway_message.mentions.len(), 1);
}

#[test]
fn rejects_channel_message_with_attachments() {
    let message = ChannelMessage {
        channel: "feishu".to_string(),
        channel_instance_id: None,
        conversation_id: "conv-4".to_string(),
        sender_id: "user-4".to_string(),
        message_id: "msg-4".to_string(),
        text: "hello".to_string(),
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
        attachments: vec![ChannelAttachment {
            kind: "file".to_string(),
            file_name: "demo.txt".to_string(),
            bytes: b"demo".to_vec(),
        }],
    };

    let error = build_gateway_channel_message(message).expect_err("attachments should fail fast");

    assert_eq!(error, GatewayChannelIngressError::UnsupportedAttachments);
}
