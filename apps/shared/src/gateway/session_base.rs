use crate::gateway::{AppTurnRequest, GatewayEntryContext, LlmRuntimeConfig};
use agent_types::interaction::InteractionResponse;
use serde::{Deserialize, Serialize};

pub fn channel_session_id(
    channel: &str,
    channel_instance_id: Option<&str>,
    conversation_id: &str,
) -> String {
    let scope = channel_instance_id.unwrap_or(channel);
    format!("{scope}:{conversation_id}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionOpenRequest {
    pub session_id: String,
    pub conversation_id: String,
    pub sender_id: String,
    #[serde(default)]
    pub entry: GatewayEntryContext,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    #[serde(default)]
    pub llm: Option<LlmRuntimeConfig>,
}

impl SessionOpenRequest {
    pub fn into_turn_request(self, text: String) -> AppTurnRequest {
        AppTurnRequest {
            session_id: self.session_id,
            entry: self.entry,
            channel: self.channel,
            message_id: None,
            conversation_id: self.conversation_id,
            sender_id: self.sender_id,
            text,
            channel_instance_id: self.channel_instance_id,
            channel_identity_prompt: None,
            reply_to_message_id: None,
            root_message_id: None,
            mentions: Vec::new(),
            reasoning_effort: Default::default(),
            llm: self.llm,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCloseRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCancelRequest {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInteractionRequest {
    pub session_id: String,
    pub response: InteractionResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionInput {
    Turn {
        request: AppTurnRequest,
    },
    Interaction {
        response: InteractionResponse,
    },
    InputChunk {
        stream_id: String,
        seq: u32,
        content: String,
        is_final: bool,
    },
    CancelActiveTurn,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionInputKind {
    Turn,
    Interaction,
    InputChunk,
    CancelActiveTurn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSubmitReceipt {
    pub session_id: String,
    pub accepted_kind: SessionInputKind,
}
