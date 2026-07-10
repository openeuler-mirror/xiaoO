use crate::backend::BackendForkResult;
use crate::gateway::{AppTurnRequest, GatewayEntryContext, LlmRuntimeConfig, SessionRecord};
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
    #[serde(rename = "runtime_id", alias = "session_id")]
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
            command_context: None,
            chain_depth: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCloseRequest {
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionCancelRequest {
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionForkRequest {
    pub parent_session_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub sender_id: Option<String>,
    #[serde(default)]
    pub snapshot_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionForkResult {
    pub parent: SessionRecord,
    pub child: SessionRecord,
    pub backend_fork: BackendForkResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInteractionRequest {
    #[serde(rename = "runtime_id", alias = "session_id")]
    pub session_id: String,
    pub response: InteractionResponse,
}

pub type RuntimeOpenRequest = SessionOpenRequest;
pub type RuntimeCloseRequest = SessionCloseRequest;
pub type RuntimeCancelRequest = SessionCancelRequest;
pub type RuntimeInteractionRequest = SessionInteractionRequest;

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

#[cfg(test)]
mod tests {
    use super::*;

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
        };

        let value = serde_json::to_value(&request).expect("request should serialize");
        assert_eq!(value["runtime_id"], "runtime-1");
        assert!(value.get("session_id").is_none());

        let legacy: RuntimeCloseRequest =
            serde_json::from_str(r#"{"session_id":"legacy-runtime"}"#)
                .expect("legacy session_id should deserialize");
        assert_eq!(legacy.session_id, "legacy-runtime");
    }
}
