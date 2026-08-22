use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TelegramConfig {
    pub channel_instance_id: Option<String>,
    pub event_transport: TelegramEventTransport,
    pub bot_token_env: String,
    pub webhook_secret_token: Option<String>,
    pub bot_username: Option<String>,
    pub base_url: String,
    pub polling_timeout_secs: u64,
    pub polling_limit: u16,
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
        if self.polling_timeout_secs == 0 {
            return Err(TelegramConfigError::InvalidField {
                field: "polling_timeout_secs",
                message: "must be greater than zero".to_string(),
            });
        }
        if !(1..=100).contains(&self.polling_limit) {
            return Err(TelegramConfigError::InvalidField {
                field: "polling_limit",
                message: "must be between 1 and 100".to_string(),
            });
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

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TelegramEventTransport {
    Webhook,
    Polling,
}

impl Default for TelegramEventTransport {
    fn default() -> Self {
        Self::Webhook
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
pub(crate) struct TelegramGetUpdatesRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<i64>,
    pub limit: u16,
    pub timeout: u64,
    #[serde(skip_serializing_if = "is_empty_allowed_updates")]
    pub allowed_updates: &'a [&'a str],
}

fn is_empty_allowed_updates(value: &[&str]) -> bool {
    value.is_empty()
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
