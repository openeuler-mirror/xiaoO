use crate::channels::telegram::client::{format_conversation_id, TelegramClient};
use crate::channels::telegram::types::{
    TelegramConfig, TelegramMessage, TelegramMessageEntity, TelegramUpdate,
};
use crate::channels::{
    AdapterResponse, ChannelAdapter, ChannelCapabilities, ChannelError, ChannelMention,
    ChannelMessage, ChannelMeta, ChannelResult, ChannelTextFormat,
};
use async_trait::async_trait;
use axum::http::HeaderMap;
use std::collections::HashMap;
use std::ops::Range;

const TELEGRAM_SECRET_TOKEN_HEADER: &str = "x-telegram-bot-api-secret-token";

#[derive(Debug, Clone)]
pub struct TelegramAdapter {
    client: TelegramClient,
    config: TelegramConfig,
}

impl TelegramAdapter {
    pub fn new(config: TelegramConfig) -> ChannelResult<Self> {
        config.validate().map_err(|error| ChannelError::Config {
            message: error.to_string(),
        })?;
        Ok(Self {
            client: TelegramClient::new(config.clone()),
            config,
        })
    }
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    fn channel_name(&self) -> &str {
        "telegram"
    }

    async fn handle_event(
        &self,
        headers: &HeaderMap,
        _query: &HashMap<String, String>,
        body: &[u8],
    ) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
        verify_secret_token(&self.config, headers)?;
        let update = serde_json::from_slice::<TelegramUpdate>(body).map_err(|error| {
            ChannelError::InvalidEvent {
                message: format!("invalid telegram update payload: {error}"),
            }
        })?;
        Ok((AdapterResponse::Accepted, self.handle_update(update)?))
    }

    async fn send_text(
        &self,
        conversation_id: &str,
        text: &str,
        reply_to_message_id: Option<&str>,
    ) -> ChannelResult<Option<String>> {
        self.client
            .send_text(conversation_id, text, reply_to_message_id)
            .await
    }
}

impl TelegramAdapter {
    pub(crate) fn handle_update(
        &self,
        update: TelegramUpdate,
    ) -> ChannelResult<Option<ChannelMessage>> {
        let Some(message) = update.supported_message() else {
            return Ok(None);
        };
        self.to_channel_message(message)
    }

    fn to_channel_message(
        &self,
        message: TelegramMessage,
    ) -> ChannelResult<Option<ChannelMessage>> {
        let Some(raw_text) = message
            .text
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let text = normalize_incoming_text(
            raw_text,
            &message.entities,
            self.config.normalized_bot_username().as_deref(),
        );
        if text.is_empty() {
            return Ok(None);
        }

        let sender_id = sender_id(&message);
        let conversation_id = format_conversation_id(message.chat.id, message.message_thread_id);
        let reply_to_message_id = message
            .reply_to_message
            .as_ref()
            .map(|reply| reply.message_id.to_string());
        let root_message_id = reply_to_message_id.clone();
        let mentions = resolve_mentions(raw_text, &message.entities);

        Ok(Some(ChannelMessage {
            channel: "telegram".to_string(),
            channel_instance_id: self.config.channel_instance_id.clone(),
            conversation_id,
            sender_id,
            message_id: message.message_id.to_string(),
            text,
            reply_to_message_id,
            root_message_id,
            mentions,
            attachments: Vec::new(),
        }))
    }
}

pub fn meta() -> ChannelMeta {
    ChannelMeta {
        id: "telegram".to_string(),
        label: "Telegram".to_string(),
        selection_label: "Telegram".to_string(),
        docs_path: "/channels/telegram".to_string(),
        docs_label: "telegram".to_string(),
        blurb: "Telegram Bot API adapter.".to_string(),
        aliases: vec!["tg".to_string()],
        order: 90,
    }
}

pub fn capabilities() -> ChannelCapabilities {
    ChannelCapabilities {
        supports_webhook: true,
        supports_direct_messages: true,
        supports_group_messages: true,
        requires_async_processing: true,
        supports_threads: true,
        supports_media: false,
        supports_member_listing: false,
        supports_reactions: false,
        supports_progress_updates: false,
        text_reply_format: ChannelTextFormat::PlainText,
    }
}

fn verify_secret_token(config: &TelegramConfig, headers: &HeaderMap) -> ChannelResult<()> {
    let Some(expected) = config.webhook_secret_token.as_deref() else {
        return Ok(());
    };
    let actual = headers
        .get(TELEGRAM_SECRET_TOKEN_HEADER)
        .ok_or_else(|| ChannelError::Authentication {
            message: "missing telegram webhook secret token".to_string(),
        })?
        .to_str()
        .map_err(|_| ChannelError::Authentication {
            message: "invalid telegram webhook secret token header".to_string(),
        })?;

    if actual != expected {
        return Err(ChannelError::Authentication {
            message: "invalid telegram webhook secret token".to_string(),
        });
    }
    Ok(())
}

fn sender_id(message: &TelegramMessage) -> String {
    if let Some(from) = message.from.as_ref() {
        return from.id.to_string();
    }
    if let Some(sender_chat) = message.sender_chat.as_ref() {
        return sender_chat.id.to_string();
    }
    message.chat.id.to_string()
}

fn normalize_incoming_text(
    text: &str,
    entities: &[TelegramMessageEntity],
    bot_username: Option<&str>,
) -> String {
    let Some(range) = leading_bot_invocation_range(text, entities, bot_username) else {
        return text.trim().to_string();
    };
    let after = text[range.end..].trim_start();
    after.trim().to_string()
}

fn leading_bot_invocation_range(
    text: &str,
    entities: &[TelegramMessageEntity],
    bot_username: Option<&str>,
) -> Option<Range<usize>> {
    let bot_username = bot_username?;
    let entity = entities
        .iter()
        .filter(|entity| entity.offset == 0)
        .min_by_key(|entity| entity.length)?;
    let range = utf16_span_to_byte_range(text, entity.offset, entity.length)?;
    let fragment = &text[range.clone()];
    match entity.kind.as_str() {
        "mention" if fragment_matches_bot_username(fragment, bot_username) => Some(range),
        "bot_command" if command_targets_bot(fragment, bot_username) => Some(range),
        _ => None,
    }
}

fn fragment_matches_bot_username(fragment: &str, bot_username: &str) -> bool {
    let normalized = fragment.trim().trim_start_matches('@');
    normalized.eq_ignore_ascii_case(bot_username.trim().trim_start_matches('@'))
}

fn command_targets_bot(command: &str, bot_username: &str) -> bool {
    let Some((_command_name, target_username)) = command.trim().split_once('@') else {
        return false;
    };
    target_username.eq_ignore_ascii_case(bot_username.trim().trim_start_matches('@'))
}

fn resolve_mentions(text: &str, entities: &[TelegramMessageEntity]) -> Vec<ChannelMention> {
    entities
        .iter()
        .filter_map(|entity| match entity.kind.as_str() {
            "mention" => {
                let range = utf16_span_to_byte_range(text, entity.offset, entity.length)?;
                let display_name = text[range].to_string();
                let id = display_name.trim().trim_start_matches('@').to_string();
                if id.is_empty() {
                    None
                } else {
                    Some(ChannelMention {
                        id,
                        display_name: Some(display_name),
                    })
                }
            }
            "text_mention" => {
                let user = entity.user.as_ref()?;
                Some(ChannelMention {
                    id: user.id.to_string(),
                    display_name: user.display_name(),
                })
            }
            _ => None,
        })
        .collect()
}

fn utf16_span_to_byte_range(text: &str, offset: usize, length: usize) -> Option<Range<usize>> {
    let end = offset.checked_add(length)?;
    let mut positions = Vec::with_capacity(text.len() + 1);
    positions.push((0_usize, 0_usize));
    let mut utf16_units = 0_usize;
    for (byte_index, ch) in text.char_indices() {
        utf16_units += ch.len_utf16();
        positions.push((utf16_units, byte_index + ch.len_utf8()));
    }
    let start_byte = positions
        .iter()
        .find_map(|(units, byte)| (*units == offset).then_some(*byte))?;
    let end_byte = positions
        .iter()
        .find_map(|(units, byte)| (*units == end).then_some(*byte))?;
    (start_byte <= end_byte).then_some(start_byte..end_byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue};

    fn config() -> TelegramConfig {
        TelegramConfig {
            channel_instance_id: Some("ops-telegram".to_string()),
            event_transport: crate::channels::telegram::types::TelegramEventTransport::Webhook,
            bot_token_env: "TELEGRAM_BOT_TOKEN".to_string(),
            webhook_secret_token: Some("secret_token-1".to_string()),
            bot_username: Some("xiaoO_bot".to_string()),
            base_url: "https://api.telegram.org".to_string(),
            polling_timeout_secs: 50,
            polling_limit: 100,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn parses_text_message_update() {
        let adapter = TelegramAdapter::new(config()).expect("config should be valid");
        let mut headers = HeaderMap::new();
        headers.insert(
            TELEGRAM_SECRET_TOKEN_HEADER,
            HeaderValue::from_static("secret_token-1"),
        );
        let body = serde_json::json!({
            "update_id": 1000,
            "message": {
                "message_id": 55,
                "message_thread_id": 7,
                "from": {
                    "id": 42,
                    "is_bot": false,
                    "first_name": "Ada"
                },
                "chat": {
                    "id": -100123,
                    "type": "supergroup",
                    "title": "Ops"
                },
                "text": "@xiaoO_bot deploy service",
                "entities": [{
                    "type": "mention",
                    "offset": 0,
                    "length": 10
                }]
            }
        })
        .to_string();

        let (response, message) = adapter
            .handle_event(&headers, &HashMap::new(), body.as_bytes())
            .await
            .expect("telegram update should parse");

        assert_eq!(response, AdapterResponse::Accepted);
        let message = message.expect("message should be emitted");
        assert_eq!(message.channel, "telegram");
        assert_eq!(message.channel_instance_id.as_deref(), Some("ops-telegram"));
        assert_eq!(message.conversation_id, "-100123:7");
        assert_eq!(message.sender_id, "42");
        assert_eq!(message.message_id, "55");
        assert_eq!(message.text, "deploy service");
        assert_eq!(message.mentions.len(), 1);
        assert_eq!(message.mentions[0].id, "xiaoO_bot");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn rejects_invalid_secret_token() {
        let adapter = TelegramAdapter::new(config()).expect("config should be valid");
        let body = serde_json::json!({
            "update_id": 1000,
            "message": {
                "message_id": 55,
                "from": { "id": 42, "is_bot": false, "first_name": "Ada" },
                "chat": { "id": 42, "type": "private" },
                "text": "hello"
            }
        })
        .to_string();

        let error = adapter
            .handle_event(&HeaderMap::new(), &HashMap::new(), body.as_bytes())
            .await
            .expect_err("missing secret token should fail");

        assert!(matches!(error, ChannelError::Authentication { .. }));
    }

    #[test]
    fn handles_utf16_entity_offsets() {
        let text = "@xiaoO_bot 🙂 hi";
        let range = utf16_span_to_byte_range(text, 11, 2).expect("emoji range should resolve");
        assert_eq!(&text[range], "🙂");
    }

    #[test]
    fn strips_bot_command_targeted_at_configured_bot() {
        let entities = vec![TelegramMessageEntity {
            kind: "bot_command".to_string(),
            offset: 0,
            length: 14,
            user: None,
        }];

        assert_eq!(
            normalize_incoming_text("/ask@xiaoO_bot status", &entities, Some("xiaoO_bot")),
            "status"
        );
        assert_eq!(
            normalize_incoming_text("/ask@other_bot status", &entities, Some("xiaoO_bot")),
            "/ask@other_bot status"
        );
    }
}
