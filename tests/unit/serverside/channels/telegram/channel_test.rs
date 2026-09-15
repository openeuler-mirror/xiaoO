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
