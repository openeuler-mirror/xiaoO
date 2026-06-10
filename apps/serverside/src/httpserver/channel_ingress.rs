use crate::channels::ChannelMessage;
use crate::gateway::{channel_session_id, AppTurnRequest, GatewayEntryContext, TurnMention};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayChannelMention {
    pub id: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayChannelMessage {
    pub channel: String,
    pub channel_instance_id: Option<String>,
    pub conversation_id: String,
    pub sender_id: String,
    pub agent_preset_id: Option<String>,
    pub message_id: String,
    pub text: String,
    pub channel_identity_prompt: Option<String>,
    pub reply_to_message_id: Option<String>,
    pub root_message_id: Option<String>,
    pub mentions: Vec<GatewayChannelMention>,
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum GatewayChannelIngressError {
    #[error("gateway channel ingress does not yet support channel attachments")]
    UnsupportedAttachments,
}

pub fn build_gateway_channel_message(
    message: ChannelMessage,
) -> Result<GatewayChannelMessage, GatewayChannelIngressError> {
    let ChannelMessage {
        channel,
        channel_instance_id,
        conversation_id,
        sender_id,
        message_id,
        text,
        reply_to_message_id,
        root_message_id,
        mentions,
        attachments,
    } = message;

    if !attachments.is_empty() {
        return Err(GatewayChannelIngressError::UnsupportedAttachments);
    }

    Ok(GatewayChannelMessage {
        channel,
        channel_instance_id,
        conversation_id,
        sender_id,
        agent_preset_id: None,
        message_id,
        text,
        channel_identity_prompt: None,
        reply_to_message_id,
        root_message_id,
        mentions: mentions
            .into_iter()
            .map(|mention| GatewayChannelMention {
                id: mention.id,
                display_name: mention.display_name,
            })
            .collect(),
    })
}

pub fn build_channel_turn_request(message: &GatewayChannelMessage) -> AppTurnRequest {
    AppTurnRequest {
        session_id: channel_session_id(
            &message.channel,
            message.channel_instance_id.as_deref(),
            &message.conversation_id,
        ),
        entry: GatewayEntryContext {
            runtime_profile_id: message.agent_preset_id.clone(),
            ..GatewayEntryContext::channel(message.channel_instance_id.clone())
        },
        channel: Some(message.channel.clone()),
        message_id: Some(message.message_id.clone()),
        conversation_id: message.conversation_id.clone(),
        sender_id: message.sender_id.clone(),
        text: message.text.clone(),
        channel_instance_id: message.channel_instance_id.clone(),
        channel_identity_prompt: message.channel_identity_prompt.clone(),
        reply_to_message_id: message.reply_to_message_id.clone(),
        root_message_id: message.root_message_id.clone(),
        mentions: message
            .mentions
            .iter()
            .map(|mention| TurnMention {
                id: mention.id.clone(),
                display_name: mention.display_name.clone(),
            })
            .collect(),
        reasoning_effort: Default::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_channel_turn_request, build_gateway_channel_message, GatewayChannelIngressError,
        GatewayChannelMention, GatewayChannelMessage,
    };
    use crate::channels::{ChannelAttachment, ChannelMention, ChannelMessage};

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

        let gateway_message =
            build_gateway_channel_message(message).expect("message should convert");

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

        let error =
            build_gateway_channel_message(message).expect_err("attachments should fail fast");

        assert_eq!(error, GatewayChannelIngressError::UnsupportedAttachments);
    }
}
