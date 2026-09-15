use crate::channels::ChannelMessage;
use thiserror::Error;
use xiaoo_shared::gateway::{
    channel_session_id, daemon_channel_principal, AppTurnRequest, GatewayEntryContext, TurnMention,
};

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
        reasoning_effort: None,
        llm: None,
        workspace: None,
        skills: None,
        command_context: None,
        chain_depth: 0,
        client_id: Some(daemon_channel_principal(&message.channel)),
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/serverside/httpserver/channel_ingress_test.rs"]
mod tests;
