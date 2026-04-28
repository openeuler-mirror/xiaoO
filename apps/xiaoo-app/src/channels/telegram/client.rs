use crate::channels::telegram::types::{
    TelegramApiResponse, TelegramConfig, TelegramReplyParameters, TelegramSendMessageRequest,
    TelegramSentMessage,
};
use crate::channels::{ChannelError, ChannelResult};
use reqwest::Client;

#[derive(Debug, Clone)]
pub struct TelegramClient {
    config: TelegramConfig,
    client: Client,
}

impl TelegramClient {
    pub fn new(config: TelegramConfig) -> Self {
        Self {
            config,
            client: Client::new(),
        }
    }

    pub async fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
        reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        let target = parse_conversation_id(conversation_id)?;
        let reply_parameters = reply_to_message_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(parse_message_id)
            .transpose()?
            .map(|message_id| TelegramReplyParameters { message_id });

        let url = self.bot_api_url("sendMessage")?;
        let response = self
            .client
            .post(url)
            .json(&TelegramSendMessageRequest {
                chat_id: target.chat_id,
                message_thread_id: target.message_thread_id,
                text,
                reply_parameters,
            })
            .send()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("telegram sendMessage request failed: {error}"),
            })?;

        let sent_message =
            validate_api_response::<TelegramSentMessage>(response, "telegram sendMessage").await?;
        Ok(Some(sent_message.message_id.to_string()))
    }

    fn bot_api_url(&self, method: &str) -> ChannelResult<String> {
        let token = self.bot_token()?;
        Ok(format!(
            "{}/bot{}/{}",
            self.config.base_url(),
            token,
            method.trim_start_matches('/')
        ))
    }

    fn bot_token(&self) -> ChannelResult<String> {
        let env_name = self.config.bot_token_env.trim();
        let token = std::env::var(env_name).map_err(|_| ChannelError::Config {
            message: format!("environment variable `{env_name}` is not set"),
        })?;
        let trimmed = token.trim();
        if trimmed.is_empty() {
            return Err(ChannelError::Config {
                message: format!("environment variable `{env_name}` is empty"),
            });
        }
        Ok(trimmed.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TelegramConversationTarget {
    chat_id: i64,
    message_thread_id: Option<i64>,
}

pub(crate) fn format_conversation_id(chat_id: i64, message_thread_id: Option<i64>) -> String {
    match message_thread_id {
        Some(message_thread_id) => format!("{chat_id}:{message_thread_id}"),
        None => chat_id.to_string(),
    }
}

fn parse_conversation_id(value: &str) -> ChannelResult<TelegramConversationTarget> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ChannelError::Delivery {
            message: "telegram conversation id is empty".to_string(),
        });
    }

    let (chat_id, message_thread_id) = match trimmed.split_once(':') {
        Some((chat_id, message_thread_id)) => {
            let chat_id = parse_chat_id(chat_id)?;
            let message_thread_id = parse_message_thread_id(message_thread_id)?;
            (chat_id, Some(message_thread_id))
        }
        None => (parse_chat_id(trimmed)?, None),
    };

    Ok(TelegramConversationTarget {
        chat_id,
        message_thread_id,
    })
}

fn parse_chat_id(value: &str) -> ChannelResult<i64> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|error| ChannelError::Delivery {
            message: format!("invalid telegram chat id `{}`: {error}", value.trim()),
        })
}

fn parse_message_thread_id(value: &str) -> ChannelResult<i64> {
    value
        .trim()
        .parse::<i64>()
        .map_err(|error| ChannelError::Delivery {
            message: format!(
                "invalid telegram message thread id `{}`: {error}",
                value.trim()
            ),
        })
}

fn parse_message_id(value: &str) -> ChannelResult<i64> {
    value
        .parse::<i64>()
        .map_err(|error| ChannelError::Delivery {
            message: format!("invalid telegram message id `{value}`: {error}"),
        })
}

async fn validate_api_response<T>(response: reqwest::Response, operation: &str) -> ChannelResult<T>
where
    T: serde::de::DeserializeOwned,
{
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to read {operation} response: {error}"),
        })?;
    if !status.is_success() {
        return Err(ChannelError::Delivery {
            message: format!(
                "{operation} failed with status {status}: {}",
                summarize_response_body(&body)
            ),
        });
    }

    let payload = serde_json::from_str::<TelegramApiResponse>(&body).map_err(|error| {
        ChannelError::Transport {
            message: format!("invalid {operation} response: {error}"),
        }
    })?;
    if !payload.ok {
        let description = payload
            .description
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("unknown error");
        return Err(ChannelError::Delivery {
            message: match payload.error_code {
                Some(error_code) => format!("{operation} failed ({error_code}): {description}"),
                None => format!("{operation} failed: {description}"),
            },
        });
    }

    let result = payload.result.ok_or_else(|| ChannelError::Delivery {
        message: format!("{operation} response missing result"),
    })?;
    serde_json::from_value::<T>(result).map_err(|error| ChannelError::Transport {
        message: format!("invalid {operation} result: {error}"),
    })
}

fn summarize_response_body(body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        return "empty response body".to_string();
    }
    let chars = body.chars().collect::<Vec<_>>();
    let preview = chars.iter().take(200).collect::<String>();
    if chars.len() > 200 {
        format!("{preview}...")
    } else {
        preview
    }
}

#[cfg(test)]
mod tests {
    use super::{format_conversation_id, parse_conversation_id};

    #[test]
    fn formats_topic_conversation_id() {
        assert_eq!(format_conversation_id(-100123, Some(42)), "-100123:42");
        assert_eq!(format_conversation_id(123, None), "123");
    }

    #[test]
    fn parses_topic_conversation_id() {
        let parsed = parse_conversation_id("-100123:42").expect("conversation id should parse");
        assert_eq!(parsed.chat_id, -100123);
        assert_eq!(parsed.message_thread_id, Some(42));
    }
}
