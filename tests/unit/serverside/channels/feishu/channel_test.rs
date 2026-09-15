use super::*;
use crate::channels::FeishuEventTransport;

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

#[tokio::test(flavor = "current_thread")]
async fn handles_url_verification_challenge() {
    let adapter = FeishuAdapter::new(config()).expect("valid config");
    let body = serde_json::json!({
        "type": "url_verification",
        "challenge": "challenge-token",
        "token": "verify_token"
    })
    .to_string();

    let (response, message) = adapter
        .handle_event(&HeaderMap::new(), &HashMap::new(), body.as_bytes())
        .await
        .expect("challenge should succeed");

    assert_eq!(
        response,
        AdapterResponse::Challenge {
            challenge: "challenge-token".to_string()
        }
    );
    assert!(message.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn parses_reply_chain_metadata() {
    let adapter = FeishuAdapter::new(config()).expect("valid config");
    let body = serde_json::json!({
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
                    "id": {
                        "open_id": "ou_target"
                    },
                    "name": "李四"
                }]
            }
        }
    })
    .to_string();

    let (response, message) = adapter
        .handle_event(&HeaderMap::new(), &HashMap::new(), body.as_bytes())
        .await
        .expect("message should succeed");

    assert_eq!(response, AdapterResponse::Accepted);
    let message = message.expect("message should exist");
    assert_eq!(message.message_id, "om_current");
    assert_eq!(message.reply_to_message_id.as_deref(), Some("om_parent"));
    assert_eq!(message.root_message_id.as_deref(), Some("om_root"));
    assert_eq!(message.mentions.len(), 1);
    assert_eq!(message.mentions[0].id, "ou_target");
}

#[tokio::test(flavor = "current_thread")]
async fn normalizes_empty_reply_chain_metadata_to_none() {
    let adapter = FeishuAdapter::new(config()).expect("valid config");
    let body = serde_json::json!({
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
                "message_id": "om_top_level",
                "chat_id": "oc_test",
                "root_id": "",
                "parent_id": "   ",
                "message_type": "text",
                "content": "{\"text\":\"你好\"}"
            }
        }
    })
    .to_string();

    let (response, message) = adapter
        .handle_event(&HeaderMap::new(), &HashMap::new(), body.as_bytes())
        .await
        .expect("message should succeed");

    assert_eq!(response, AdapterResponse::Accepted);
    let message = message.expect("message should exist");
    assert_eq!(message.message_id, "om_top_level");
    assert!(message.reply_to_message_id.is_none());
    assert!(message.root_message_id.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn strips_repeated_leading_invocation_mentions() {
    let adapter = FeishuAdapter::new(config()).expect("valid config");
    let body = serde_json::json!({
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
                "message_id": "om_repeat",
                "chat_id": "oc_test",
                "message_type": "text",
                "content": "{\"text\":\"@@_user_1 现在拥有哪些能力\"}",
                "mentions": [{
                    "key": "@_user_1",
                    "id": {
                        "open_id": "ou_bot"
                    },
                    "name": "小欧 beta 0.92"
                }]
            }
        }
    })
    .to_string();

    let (_response, message) = adapter
        .handle_event(&HeaderMap::new(), &HashMap::new(), body.as_bytes())
        .await
        .expect("message should succeed");

    let message = message.expect("message should exist");
    assert_eq!(message.text, "现在拥有哪些能力");
    assert_eq!(message.mentions.len(), 1);
    assert_eq!(message.mentions[0].id, "ou_bot");
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_invalid_verification_token() {
    let adapter = FeishuAdapter::new(config()).expect("valid config");
    let body = serde_json::json!({
        "schema": "2.0",
        "header": {
            "event_type": "im.message.receive_v1",
            "token": "wrong"
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
                "content": "{\"text\":\"hello\"}"
            }
        }
    })
    .to_string();

    let error = adapter
        .handle_event(&HeaderMap::new(), &HashMap::new(), body.as_bytes())
        .await
        .expect_err("message should fail");

    assert!(matches!(error, ChannelError::Authentication { .. }));
}
