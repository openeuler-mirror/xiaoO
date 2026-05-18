use crate::channels::feishu::types::{
    FeishuChallengeEvent, FeishuConfig, FeishuEventEnvelope, FeishuMention, FeishuMessage,
    FeishuTextContent, FeishuWebhookEvent,
};
use crate::channels::{
    AdapterResponse, ChannelError, ChannelMention, ChannelMessage, ChannelResult,
};
use regex::Regex;
use std::sync::OnceLock;

pub(crate) fn handle_event(
    config: &FeishuConfig,
    body: &[u8],
) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
    let envelope = serde_json::from_slice::<FeishuEventEnvelope>(body).map_err(|error| {
        ChannelError::InvalidEvent {
            message: error.to_string(),
        }
    })?;

    match envelope {
        FeishuEventEnvelope::Challenge(challenge) => handle_challenge(config, challenge),
        FeishuEventEnvelope::Event(event) => handle_webhook_event(config, event),
    }
}

pub(crate) fn handle_long_connection_payload(
    config: &FeishuConfig,
    body: &[u8],
) -> ChannelResult<Option<ChannelMessage>> {
    let event = serde_json::from_slice::<FeishuWebhookEvent>(body).map_err(|error| {
        ChannelError::InvalidEvent {
            message: error.to_string(),
        }
    })?;

    let verify_verification_token = config
        .verification_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some()
        && event
            .header
            .token
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_some();

    handle_webhook_event_body(config, event, verify_verification_token)
        .map(|(_response, message)| message)
}

fn handle_challenge(
    config: &FeishuConfig,
    challenge: FeishuChallengeEvent,
) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
    if challenge.event_type != "url_verification" {
        return Err(ChannelError::InvalidEvent {
            message: format!("unsupported challenge type `{}`", challenge.event_type),
        });
    }
    verify_token(config, challenge.token.as_deref())?;
    Ok((
        AdapterResponse::Challenge {
            challenge: challenge.challenge,
        },
        None,
    ))
}

fn handle_webhook_event(
    config: &FeishuConfig,
    event: FeishuWebhookEvent,
) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
    handle_webhook_event_body(config, event, true)
}

fn handle_webhook_event_body(
    config: &FeishuConfig,
    event: FeishuWebhookEvent,
    verify_verification_token: bool,
) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
    if verify_verification_token {
        verify_token(config, event.header.token.as_deref())?;
    }
    if event.header.event_type != "im.message.receive_v1" {
        return Ok((AdapterResponse::Accepted, None));
    }

    let sender = event
        .event
        .sender
        .and_then(|sender| sender.sender_id)
        .ok_or_else(|| ChannelError::InvalidEvent {
            message: "missing feishu sender".to_string(),
        })?;
    let sender_id =
        sender
            .open_id
            .or(sender.user_id)
            .ok_or_else(|| ChannelError::InvalidEvent {
                message: "missing feishu sender id".to_string(),
            })?;
    let message = event
        .event
        .message
        .ok_or_else(|| ChannelError::InvalidEvent {
            message: "missing feishu message body".to_string(),
        })?;

    match message.message_type.as_str() {
        "text" => Ok((
            AdapterResponse::Accepted,
            Some(parse_text_message(config, message, sender_id)?),
        )),
        "file" if config.parse_file_messages => Err(ChannelError::UnsupportedCapability {
            capability: "feishu file parsing".to_string(),
        }),
        "file" => Ok((AdapterResponse::Accepted, None)),
        _ => Ok((AdapterResponse::Accepted, None)),
    }
}

fn verify_token(config: &FeishuConfig, token: Option<&str>) -> ChannelResult<()> {
    let expected =
        config
            .verification_token
            .as_deref()
            .ok_or_else(|| ChannelError::Authentication {
                message: "missing feishu verification token configuration".to_string(),
            })?;
    let token = token.ok_or_else(|| ChannelError::Authentication {
        message: "missing feishu verification token".to_string(),
    })?;
    if token != expected {
        return Err(ChannelError::Authentication {
            message: "invalid feishu verification token".to_string(),
        });
    }
    Ok(())
}

fn parse_text_message(
    config: &FeishuConfig,
    message: FeishuMessage,
    sender_id: String,
) -> ChannelResult<ChannelMessage> {
    let content = serde_json::from_str::<FeishuTextContent>(&message.content).map_err(|error| {
        ChannelError::InvalidEvent {
            message: format!("invalid feishu text content: {error}"),
        }
    })?;
    let raw_text = content.text;
    let reply_to_message_id = normalize_optional_identifier(message.parent_id);
    let root_message_id = normalize_optional_identifier(message.root_id);
    Ok(ChannelMessage {
        channel: "feishu".to_string(),
        channel_instance_id: config.channel_instance_id.clone(),
        conversation_id: message.chat_id,
        sender_id,
        message_id: message.message_id,
        text: normalize_leading_feishu_invocation_text(&raw_text, &message.mentions),
        reply_to_message_id,
        root_message_id,
        mentions: resolve_mentions(&raw_text, &message.mentions),
        attachments: Vec::new(),
    })
}

fn normalize_optional_identifier(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn resolve_mentions(text: &str, mentions: &[FeishuMention]) -> Vec<ChannelMention> {
    let mentions = extract_mentions_from_payload(mentions);
    if mentions.is_empty() {
        extract_mentions_from_text(text)
    } else {
        mentions
    }
}

fn extract_mentions_from_payload(mentions: &[FeishuMention]) -> Vec<ChannelMention> {
    mentions
        .iter()
        .filter_map(|mention| {
            let id = mention
                .id
                .as_ref()
                .and_then(|id| {
                    id.open_id
                        .as_ref()
                        .or(id.user_id.as_ref())
                        .or(id.union_id.as_ref())
                })?
                .clone();

            Some(ChannelMention {
                id,
                display_name: mention.name.clone(),
            })
        })
        .collect()
}

fn extract_mentions_from_text(text: &str) -> Vec<ChannelMention> {
    static MENTION_PATTERN: OnceLock<Regex> = OnceLock::new();
    let pattern = MENTION_PATTERN.get_or_init(|| {
        Regex::new(r#"<at[^>]*user_id="([^"]+)"[^>]*>(.*?)</at>"#)
            .expect("valid feishu mention regex")
    });

    pattern
        .captures_iter(text)
        .filter_map(|capture| {
            let id = capture.get(1)?.as_str().trim();
            if id.is_empty() {
                return None;
            }

            let display_name = capture
                .get(2)
                .map(|value| value.as_str().trim().to_string());
            Some(ChannelMention {
                id: id.to_string(),
                display_name: display_name.filter(|value| !value.is_empty()),
            })
        })
        .collect()
}

fn normalize_leading_feishu_invocation_text(text: &str, mentions: &[FeishuMention]) -> String {
    let mention_keys = mentions
        .iter()
        .filter_map(|mention| mention.key.as_deref())
        .map(str::trim)
        .filter(|key| !key.is_empty())
        .collect::<Vec<_>>();
    let invocation_mention_ids = mentions
        .iter()
        .filter(|mention| {
            mention
                .key
                .as_deref()
                .map(str::trim)
                .filter(|key| !key.is_empty())
                .is_some()
        })
        .filter_map(|mention| mention.id.as_ref())
        .filter_map(|id| {
            id.open_id
                .as_deref()
                .or(id.user_id.as_deref())
                .or(id.union_id.as_deref())
        })
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();

    let mut remaining = text.trim_start();
    let mut consumed_any = false;

    while let Some(next) =
        strip_one_leading_feishu_invocation(remaining, &mention_keys, &invocation_mention_ids)
    {
        consumed_any = true;
        remaining = trim_leading_invocation_separators(next);
    }

    if consumed_any {
        remaining.to_string()
    } else {
        text.to_string()
    }
}

fn strip_one_leading_feishu_invocation<'a>(
    text: &'a str,
    mention_keys: &[&str],
    invocation_mention_ids: &[&str],
) -> Option<&'a str> {
    let trimmed = text.trim_start();

    if let Some(after) = strip_leading_feishu_at_tag_for_ids(trimmed, invocation_mention_ids) {
        return Some(after);
    }

    for key in mention_keys {
        if let Some(after) = strip_leading_feishu_mention_key(trimmed, key) {
            return Some(after);
        }
    }

    None
}

fn strip_leading_feishu_at_tag_for_ids<'a>(
    text: &'a str,
    invocation_mention_ids: &[&str],
) -> Option<&'a str> {
    if !text.starts_with("<at") || invocation_mention_ids.is_empty() {
        return None;
    }

    let tag_end = text.find('>')?;
    let tag = text.get(..=tag_end)?;
    let closing = text.find("</at>")?;
    let after = text.get(closing + "</at>".len()..)?;

    static USER_ID_PATTERN: OnceLock<Regex> = OnceLock::new();
    let capture = USER_ID_PATTERN
        .get_or_init(|| Regex::new(r#"user_id="([^"]+)""#).expect("valid feishu at tag regex"));
    let user_id = capture
        .captures(tag)
        .and_then(|caps| caps.get(1))
        .map(|value| value.as_str().trim())?;

    if invocation_mention_ids.iter().any(|id| *id == user_id) {
        Some(after)
    } else {
        None
    }
}

fn strip_leading_feishu_mention_key<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }

    if let Some(after) = text.strip_prefix(key) {
        return Some(after);
    }

    let key_without_at = key.strip_prefix('@')?;
    let leading_ats = text.bytes().take_while(|byte| *byte == b'@').count();
    if leading_ats == 0 {
        return None;
    }

    let after_ats = text.get(leading_ats..)?;
    if !after_ats.starts_with(key_without_at) {
        return None;
    }

    text.get(leading_ats + key_without_at.len()..)
}

fn trim_leading_invocation_separators(text: &str) -> &str {
    text.trim_start_matches(|ch: char| {
        ch.is_whitespace() || matches!(ch, '@' | ',' | '，' | ':' | '：' | ';' | '；')
    })
}

#[cfg(test)]
mod tests {
    use super::{handle_event, handle_long_connection_payload};
    use crate::channels::feishu::{FeishuConfig, FeishuEventTransport};
    use crate::channels::AdapterResponse;

    fn config() -> FeishuConfig {
        FeishuConfig {
            channel_instance_id: Some("ops-feishu".to_string()),
            base_url: "https://open.feishu.cn".to_string(),
            app_id: "cli_test".to_string(),
            app_secret_env: "FEISHU_APP_SECRET".to_string(),
            event_transport: FeishuEventTransport::Webhook,
            verification_token: Some("verify_token".to_string()),
            parse_file_messages: false,
            max_file_download_bytes: 0,
            max_file_text_chars: 0,
        }
    }

    #[test]
    fn handles_url_verification_challenge() {
        let payload = serde_json::json!({
            "type": "url_verification",
            "challenge": "challenge-token",
            "token": "verify_token"
        });

        let (response, message) =
            handle_event(&config(), payload.to_string().as_bytes()).expect("challenge should pass");

        assert_eq!(
            response,
            AdapterResponse::Challenge {
                challenge: "challenge-token".to_string()
            }
        );
        assert!(message.is_none());
    }

    #[test]
    fn parses_webhook_text_message() {
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "verify_token"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_current",
                    "chat_id": "oc_test",
                    "root_id": "om_root",
                    "parent_id": "om_parent",
                    "message_type": "text",
                    "content": "{\"text\":\"<at user_id=\\\"ou_target\\\">李四</at> 请看这条\"}",
                    "mentions": [{
                        "key": "@小欧",
                        "id": {
                            "open_id": "ou_target"
                        },
                        "name": "李四"
                    }]
                }
            }
        });

        let (response, message) =
            handle_event(&config(), payload.to_string().as_bytes()).expect("event should parse");

        assert_eq!(response, AdapterResponse::Accepted);
        let message = message.expect("text message should be produced");
        assert_eq!(message.text, "请看这条");
        assert_eq!(message.sender_id, "ou_sender");
        assert_eq!(message.reply_to_message_id.as_deref(), Some("om_parent"));
        assert_eq!(message.root_message_id.as_deref(), Some("om_root"));
        assert_eq!(message.mentions.len(), 1);
    }

    #[test]
    fn parses_long_connection_payload_without_token_check() {
        let config = FeishuConfig {
            event_transport: FeishuEventTransport::Websocket,
            verification_token: None,
            ..config()
        };
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_current",
                    "chat_id": "oc_test",
                    "message_type": "text",
                    "content": "{\"text\":\"你好\"}",
                    "mentions": []
                }
            }
        });

        let message = handle_long_connection_payload(&config, payload.to_string().as_bytes())
            .expect("persistent payload should parse")
            .expect("text message should be produced");

        assert_eq!(message.text, "你好");
        assert_eq!(message.sender_id, "ou_sender");
        assert_eq!(message.reply_to_message_id, None);
        assert_eq!(message.root_message_id, None);
    }

    #[test]
    fn long_connection_payload_accepts_missing_token_when_configured() {
        let config = FeishuConfig {
            event_transport: FeishuEventTransport::Websocket,
            verification_token: Some("verify_token".to_string()),
            ..config()
        };
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_current",
                    "chat_id": "oc_test",
                    "message_type": "text",
                    "content": "{\"text\":\"你好\"}",
                    "mentions": []
                }
            }
        });

        let message = handle_long_connection_payload(&config, payload.to_string().as_bytes())
            .expect("long connection auth is established by the websocket endpoint")
            .expect("text message should be produced");

        assert_eq!(message.text, "你好");
    }

    #[test]
    fn long_connection_payload_rejects_mismatched_present_token() {
        let config = FeishuConfig {
            event_transport: FeishuEventTransport::Websocket,
            verification_token: Some("verify_token".to_string()),
            ..config()
        };
        let payload = serde_json::json!({
            "schema": "2.0",
            "header": {
                "event_type": "im.message.receive_v1",
                "token": "wrong_token"
            },
            "event": {
                "sender": {
                    "sender_id": {
                        "open_id": "ou_sender"
                    }
                },
                "message": {
                    "message_id": "om_current",
                    "chat_id": "oc_test",
                    "message_type": "text",
                    "content": "{\"text\":\"你好\"}",
                    "mentions": []
                }
            }
        });

        let error = handle_long_connection_payload(&config, payload.to_string().as_bytes())
            .expect_err("mismatched websocket token should be rejected when payload includes one");

        assert!(matches!(
            error,
            crate::channels::ChannelError::Authentication { .. }
        ));
    }
}
