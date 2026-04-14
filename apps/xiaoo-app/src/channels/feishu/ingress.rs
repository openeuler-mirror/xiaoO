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
    verify_token(config, event.header.token.as_deref())?;
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
    let token = token.ok_or_else(|| ChannelError::Authentication {
        message: "missing feishu verification token".to_string(),
    })?;
    if token != config.verification_token {
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
