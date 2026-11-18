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

/// Websocket variant of the webhook config; the token policy is the input
/// under test in the long-connection payloads below.
fn long_connection_config(verification_token: Option<&str>) -> FeishuConfig {
    FeishuConfig {
        event_transport: FeishuEventTransport::Websocket,
        verification_token: verification_token.map(|token| token.to_string()),
        ..config()
    }
}

/// `im.message.receive_v1` text event; long-connection cases differ only in
/// the optional header token, which the handler must not require (the
/// websocket endpoint already authenticates) yet must still verify when the
/// payload carries one.
fn long_connection_text_payload(token: Option<&str>) -> Vec<u8> {
    let mut header = serde_json::json!({ "event_type": "im.message.receive_v1" });
    if let Some(token) = token {
        header["token"] = serde_json::json!(token);
    }
    serde_json::json!({
        "schema": "2.0",
        "header": header,
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
    })
    .to_string()
    .into_bytes()
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
    let message = handle_long_connection_payload(
        &long_connection_config(None),
        &long_connection_text_payload(None),
    )
    .expect("persistent payload should parse")
    .expect("text message should be produced");

    assert_eq!(message.text, "你好");
    assert_eq!(message.sender_id, "ou_sender");
    assert_eq!(message.reply_to_message_id, None);
    assert_eq!(message.root_message_id, None);
}

#[test]
fn long_connection_payload_accepts_missing_token_when_configured() {
    let message = handle_long_connection_payload(
        &long_connection_config(Some("verify_token")),
        &long_connection_text_payload(None),
    )
    .expect("long connection auth is established by the websocket endpoint")
    .expect("text message should be produced");

    assert_eq!(message.text, "你好");
}

#[test]
fn long_connection_payload_rejects_mismatched_present_token() {
    let error = handle_long_connection_payload(
        &long_connection_config(Some("verify_token")),
        &long_connection_text_payload(Some("wrong_token")),
    )
    .expect_err("mismatched websocket token should be rejected when payload includes one");

    assert!(matches!(
        error,
        crate::channels::ChannelError::Authentication { .. }
    ));
}
