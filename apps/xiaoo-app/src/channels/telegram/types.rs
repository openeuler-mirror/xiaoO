use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramConfig {
    pub channel_instance_id: Option<String>,
    pub bot_token_env: String,
    pub webhook_secret_token: Option<String>,
    pub bot_username: Option<String>,
    pub base_url: String,
}

impl TelegramConfig {
    pub(crate) fn base_url(&self) -> &str {
        self.base_url.trim_end_matches('/')
    }

    pub(crate) fn validate(&self) -> Result<(), TelegramConfigError> {
        if self.base_url.trim().is_empty() {
            return Err(TelegramConfigError::InvalidField {
                field: "base_url",
                message: "must not be empty".to_string(),
            });
        }
        if self.bot_token_env.trim().is_empty() {
            return Err(TelegramConfigError::InvalidField {
                field: "bot_token_env",
                message: "must not be empty".to_string(),
            });
        }
        if let Some(secret_token) = self.webhook_secret_token.as_deref() {
            let len = secret_token.len();
            if !(1..=256).contains(&len)
                || !secret_token
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            {
                return Err(TelegramConfigError::InvalidField {
                    field: "webhook_secret_token",
                    message: "must be 1-256 characters and only contain A-Z, a-z, 0-9, _ or -"
                        .to_string(),
                });
            }
        }
        if let Some(bot_username) = self.bot_username.as_deref() {
            if bot_username.trim().trim_start_matches('@').is_empty() {
                return Err(TelegramConfigError::InvalidField {
                    field: "bot_username",
                    message: "must not be empty".to_string(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn normalized_bot_username(&self) -> Option<String> {
        self.bot_username
            .as_deref()
            .map(str::trim)
            .map(|value| value.trim_start_matches('@'))
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum TelegramConfigError {
    #[error("invalid telegram config field `{field}`: {message}")]
    InvalidField {
        field: &'static str,
        message: String,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramUpdate {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<TelegramMessage>,
    #[serde(default)]
    pub channel_post: Option<TelegramMessage>,
}

impl TelegramUpdate {
    pub(crate) fn supported_message(self) -> Option<TelegramMessage> {
        self.message.or(self.channel_post)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramMessage {
    pub message_id: i64,
    #[serde(default)]
    pub message_thread_id: Option<i64>,
    #[serde(default)]
    pub from: Option<TelegramUser>,
    #[serde(default)]
    pub sender_chat: Option<TelegramChat>,
    pub chat: TelegramChat,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub entities: Vec<TelegramMessageEntity>,
    #[serde(default)]
    pub reply_to_message: Option<Box<TelegramMessage>>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct TelegramUser {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
}

impl TelegramUser {
    pub(crate) fn display_name(&self) -> Option<String> {
        let name = [self.first_name.as_deref(), self.last_name.as_deref()]
            .into_iter()
            .flatten()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        if !name.is_empty() {
            Some(name)
        } else {
            self.username
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct TelegramChat {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub(crate) struct TelegramMessageEntity {
    #[serde(rename = "type")]
    pub kind: String,
    pub offset: usize,
    pub length: usize,
    #[serde(default)]
    pub user: Option<TelegramUser>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TelegramSendMessageRequest<'a> {
    pub chat_id: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_thread_id: Option<i64>,
    pub text: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_parameters: Option<TelegramReplyParameters>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TelegramReplyParameters {
    pub message_id: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramApiResponse {
    pub ok: bool,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub error_code: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct TelegramSentMessage {
    pub message_id: i64,
}
