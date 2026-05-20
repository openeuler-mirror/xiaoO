use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeishuEventTransport {
    Webhook,
    Websocket,
}

impl Default for FeishuEventTransport {
    fn default() -> Self {
        Self::Webhook
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuConfig {
    pub channel_instance_id: Option<String>,
    pub base_url: String,
    pub app_id: String,
    pub app_secret_env: String,
    pub event_transport: FeishuEventTransport,
    pub verification_token: Option<String>,
    pub parse_file_messages: bool,
    pub max_file_download_bytes: u64,
    pub max_file_text_chars: usize,
}

impl FeishuConfig {
    pub(crate) fn base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    pub(crate) fn validate(&self) -> Result<(), FeishuConfigError> {
        if self.base_url.trim().is_empty() {
            return Err(FeishuConfigError::InvalidField {
                field: "base_url",
                message: "must not be empty".to_string(),
            });
        }
        if self.app_id.trim().is_empty() {
            return Err(FeishuConfigError::InvalidField {
                field: "app_id",
                message: "must not be empty".to_string(),
            });
        }
        if self.app_secret_env.trim().is_empty() {
            return Err(FeishuConfigError::InvalidField {
                field: "app_secret_env",
                message: "must not be empty".to_string(),
            });
        }
        if self.event_transport == FeishuEventTransport::Webhook
            && self
                .verification_token
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .is_none()
        {
            return Err(FeishuConfigError::InvalidField {
                field: "verification_token",
                message: "must not be empty for webhook transport".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum FeishuConfigError {
    #[error("invalid feishu config field `{field}`: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuSendRequest {
    pub conversation_id: String,
    pub reply_to_message_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuCardRequest {
    pub conversation_id: String,
    pub reply_to_message_id: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuChatMember {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuChatInfo {
    pub owner_id: Option<String>,
    pub chat_type: Option<String>,
    pub user_manager_ids: Vec<String>,
    pub bot_manager_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum FeishuEventEnvelope {
    Challenge(FeishuChallengeEvent),
    Event(FeishuWebhookEvent),
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuChallengeEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub challenge: String,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuWebhookEvent {
    pub _schema: Option<String>,
    pub header: FeishuEventHeader,
    pub event: FeishuEventBody,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuEventHeader {
    pub event_type: String,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuEventBody {
    pub sender: Option<FeishuSender>,
    pub message: Option<FeishuMessage>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuSender {
    pub sender_id: Option<FeishuSenderId>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuSenderId {
    #[serde(default)]
    pub open_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuMessage {
    pub message_id: String,
    pub chat_id: String,
    #[serde(default)]
    pub root_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(rename = "message_type")]
    pub message_type: String,
    pub content: String,
    #[serde(default)]
    pub mentions: Vec<FeishuMention>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct FeishuTextContent {
    pub text: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuMention {
    pub key: Option<String>,
    pub id: Option<FeishuMentionId>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct FeishuMentionId {
    #[serde(default)]
    pub open_id: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    #[serde(default)]
    pub union_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TenantTokenRequest<'a> {
    pub app_id: &'a str,
    pub app_secret: &'a str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TenantTokenResponse {
    pub code: i32,
    pub msg: Option<String>,
    pub tenant_access_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SendMessageRequest<'a> {
    pub receive_id: &'a str,
    pub msg_type: &'a str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReplyMessageRequest<'a> {
    pub msg_type: &'a str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct UpdateMessageRequest<'a> {
    pub msg_type: &'a str,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReactionRequest<'a> {
    pub reaction_type: ReactionType<'a>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ReactionType<'a> {
    pub emoji_type: &'a str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiResponse {
    pub code: i32,
    pub msg: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SendMessageResponse {
    pub code: i32,
    pub msg: Option<String>,
    pub data: Option<SendMessageData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SendMessageData {
    pub message_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatInfoEnvelope {
    pub code: i32,
    pub msg: Option<String>,
    pub data: Option<ChatInfoData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChatInfoData {
    pub owner_id: Option<String>,
    pub chat_type: Option<String>,
    #[serde(default)]
    pub user_manager_id_list: Vec<String>,
    #[serde(default)]
    pub bot_manager_id_list: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct WsEndpointRequest<'a> {
    #[serde(rename = "AppID")]
    pub app_id: &'a str,
    #[serde(rename = "AppSecret")]
    pub app_secret: &'a str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WsEndpointResponse {
    pub code: i32,
    pub msg: Option<String>,
    pub data: Option<WsEndpointData>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct WsEndpointData {
    #[serde(rename = "URL")]
    pub url: String,
    #[serde(rename = "ClientConfig")]
    pub client_config: WsEndpointClientConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct WsEndpointClientConfig {
    #[serde(rename = "PingInterval")]
    pub ping_interval_secs: u64,
    #[serde(rename = "ReconnectCount")]
    #[allow(dead_code)]
    pub reconnect_count: i64,
    #[serde(rename = "ReconnectInterval")]
    pub reconnect_interval_secs: u64,
    #[serde(rename = "ReconnectNonce")]
    pub reconnect_nonce_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WsAckPayload {
    pub code: u16,
}

#[cfg(test)]
mod tests {
    use super::{FeishuConfig, FeishuEventTransport};

    fn config(
        event_transport: FeishuEventTransport,
        verification_token: Option<&str>,
    ) -> FeishuConfig {
        FeishuConfig {
            channel_instance_id: Some("ops-feishu".to_string()),
            base_url: "https://open.feishu.cn".to_string(),
            app_id: "cli_test".to_string(),
            app_secret_env: "FEISHU_APP_SECRET".to_string(),
            event_transport,
            verification_token: verification_token.map(ToString::to_string),
            parse_file_messages: false,
            max_file_download_bytes: 0,
            max_file_text_chars: 0,
        }
    }

    #[test]
    fn webhook_transport_requires_verification_token() {
        let error = config(FeishuEventTransport::Webhook, None)
            .validate()
            .expect_err("webhook transport should require a verification token");

        assert!(matches!(
            error,
            super::FeishuConfigError::InvalidField {
                field: "verification_token",
                ..
            }
        ));
    }

    #[test]
    fn websocket_transport_allows_missing_verification_token() {
        config(FeishuEventTransport::Websocket, None)
            .validate()
            .expect("websocket transport should not require a verification token");
    }
}
