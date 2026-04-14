use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuConfig {
    pub channel_instance_id: Option<String>,
    pub base_url: String,
    pub app_id: String,
    pub app_secret_env: String,
    pub verification_token: String,
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
        if self.verification_token.trim().is_empty() {
            return Err(FeishuConfigError::InvalidField {
                field: "verification_token",
                message: "must not be empty".to_string(),
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
