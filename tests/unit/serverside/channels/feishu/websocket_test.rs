use super::{
    build_ack_frame, parse_service_id, EventChunkCache, FeishuWebsocketMessageHandler,
    FeishuWebsocketService, ProtoFrame, ProtoHeader, FRAME_TYPE_DATA, HEADER_MESSAGE_ID,
    HEADER_SEQ, HEADER_SUM, HEADER_TRACE_ID, HEADER_TYPE, MAX_EVENT_CHUNKS, MESSAGE_TYPE_EVENT,
};
use crate::channels::feishu::{FeishuConfig, FeishuEventTransport};
use futures_util::{SinkExt, StreamExt};
use prost::Message;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;
use tokio_tungstenite::{accept_async, tungstenite::protocol::Message as WsMessage};

fn websocket_config(base_url: String, app_secret_env: String) -> FeishuConfig {
    FeishuConfig {
        channel_instance_id: Some("ops-feishu".to_string()),
        base_url,
        app_id: "cli_test".to_string(),
        app_secret_env,
        event_transport: FeishuEventTransport::Websocket,
        verification_token: None,
        parse_file_messages: false,
        max_file_download_bytes: 0,
        max_file_text_chars: 0,
    }
}

#[test]
fn parses_service_id_from_endpoint_url() {
    let service_id = parse_service_id("wss://ws.example.com/socket?service_id=42&device_id=x")
        .expect("service_id should parse");
    assert_eq!(service_id, 42);
}

#[test]
fn rejects_non_websocket_endpoint_scheme() {
    let error = parse_service_id("https://ws.example.com/socket?service_id=42")
        .expect_err("endpoint URL must use a websocket scheme");

    assert!(matches!(
        error,
        crate::channels::ChannelError::Config { .. }
    ));
}

#[test]
fn merges_chunked_payloads_in_order() {
    let mut cache = EventChunkCache::default();
    assert!(cache
        .push(
            "msg".to_string(),
            "trace".to_string(),
            2,
            0,
            b"{\"a\":".to_vec()
        )
        .expect("first chunk should be accepted")
        .is_none());

    let merged = cache
        .push("msg".to_string(), "trace".to_string(), 2, 1, b"1}".to_vec())
        .expect("second chunk should merge")
        .expect("merged payload should be ready");

    assert_eq!(merged, br#"{"a":1}"#.to_vec());
}

#[test]
fn rejects_chunk_count_above_limit() {
    let mut cache = EventChunkCache::default();
    let error = cache
        .push(
            "msg".to_string(),
            "trace".to_string(),
            MAX_EVENT_CHUNKS + 1,
            0,
            b"{}".to_vec(),
        )
        .expect_err("oversized chunk count should be rejected");

    assert!(matches!(
        error,
        crate::channels::ChannelError::Transport { .. }
    ));
}

#[test]
fn rejects_duplicate_chunk_index() {
    let mut cache = EventChunkCache::default();
    assert!(cache
        .push(
            "msg".to_string(),
            "trace".to_string(),
            2,
            0,
            b"{\"a\":".to_vec()
        )
        .expect("first chunk should be accepted")
        .is_none());

    let error = cache
        .push(
            "msg".to_string(),
            "trace".to_string(),
            2,
            0,
            b"{\"b\":".to_vec(),
        )
        .expect_err("duplicate chunk should be rejected");

    assert!(matches!(
        error,
        crate::channels::ChannelError::Transport { .. }
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn receives_and_acknowledges_long_connection_event() {
    let ws_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("websocket listener should bind");
    let ws_addr = ws_listener.local_addr().expect("ws addr should exist");
    let ws_url = format!("ws://{}/socket?service_id=777&device_id=test", ws_addr);

    let http_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("http listener should bind");
    let http_addr = http_listener.local_addr().expect("http addr should exist");
    let base_url = format!("http://{}", http_addr);

    let (ack_tx, ack_rx) = oneshot::channel();
    let ws_server = tokio::spawn(async move {
        let (stream, _) = ws_listener
            .accept()
            .await
            .expect("ws accept should succeed");
        let mut socket = accept_async(stream)
            .await
            .expect("ws handshake should succeed");

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
        })
        .to_string()
        .into_bytes();
        let event_frame = ProtoFrame {
            seq_id: 1,
            log_id: 2,
            service: 777,
            method: FRAME_TYPE_DATA,
            headers: vec![
                ProtoHeader::new(HEADER_TYPE, MESSAGE_TYPE_EVENT),
                ProtoHeader::new(HEADER_MESSAGE_ID, "msg-1"),
                ProtoHeader::new(HEADER_SUM, "1"),
                ProtoHeader::new(HEADER_SEQ, "0"),
                ProtoHeader::new(HEADER_TRACE_ID, "trace-1"),
            ],
            payload_encoding: String::new(),
            payload_type: String::new(),
            payload,
            log_id_new: String::new(),
        };
        socket
            .send(WsMessage::Binary(event_frame.encode_to_vec().into()))
            .await
            .expect("event frame should send");

        while let Some(message) = socket.next().await {
            let message = message.expect("server should receive a websocket frame");
            let WsMessage::Binary(binary) = message else {
                continue;
            };
            let frame = ProtoFrame::decode(binary.as_ref()).expect("ack frame should decode");
            if frame.method != FRAME_TYPE_DATA {
                continue;
            }
            let ack = serde_json::from_slice::<crate::channels::feishu::types::WsAckPayload>(
                &frame.payload,
            )
            .expect("ack payload should decode");
            let _ = ack_tx.send(ack.code);
            break;
        }
    });

    let http_task = tokio::spawn(async move {
        let app = axum::Router::new().route(
            "/callback/ws/endpoint",
            axum::routing::post({
                let ws_url = ws_url.clone();
                move || {
                    let ws_url = ws_url.clone();
                    async move {
                        axum::Json(serde_json::json!({
                            "code": 0,
                            "data": {
                                "URL": ws_url,
                                "ClientConfig": {
                                    "PingInterval": 3600,
                                    "ReconnectCount": 0,
                                    "ReconnectInterval": 1,
                                    "ReconnectNonce": 0
                                }
                            }
                        }))
                    }
                }
            }),
        );

        axum::serve(http_listener, app)
            .await
            .expect("http mock should serve");
    });

    let env_name = format!("FEISHU_WS_SECRET_{}", uuid::Uuid::new_v4().simple());
    std::env::set_var(&env_name, "app-secret");

    let (message_tx, message_rx) = oneshot::channel();
    let message_tx = Arc::new(Mutex::new(Some(message_tx)));
    let handler: FeishuWebsocketMessageHandler = Arc::new(move |message| {
        let message_tx = message_tx.clone();
        Box::pin(async move {
            if let Some(tx) = message_tx.lock().await.take() {
                let _ = tx.send(message);
            }
        })
    });

    let service = FeishuWebsocketService::new(websocket_config(base_url, env_name.clone()))
        .expect("service should build");
    let worker = tokio::spawn(async move {
        service.run_forever(handler).await;
    });

    let received_message = timeout(std::time::Duration::from_secs(5), message_rx)
        .await
        .expect("message should be dispatched in time")
        .expect("message channel should resolve");
    let ack_code = timeout(std::time::Duration::from_secs(5), ack_rx)
        .await
        .expect("ack should be produced in time")
        .expect("ack channel should resolve");

    assert_eq!(received_message.text, "你好");
    assert_eq!(received_message.conversation_id, "oc_test");
    assert_eq!(ack_code, 200);

    worker.abort();
    ws_server.abort();
    http_task.abort();
    std::env::remove_var(env_name);
}

#[test]
fn carries_ack_payload_code() {
    let frame = ProtoFrame {
        seq_id: 1,
        log_id: 2,
        service: 3,
        method: FRAME_TYPE_DATA,
        headers: vec![ProtoHeader::new(HEADER_TYPE, MESSAGE_TYPE_EVENT)],
        payload_encoding: String::new(),
        payload_type: String::new(),
        payload: Vec::new(),
        log_id_new: String::new(),
    };

    let ack = build_ack_frame(&frame, 200, std::time::Duration::from_millis(12))
        .expect("ack frame should build");
    let payload =
        serde_json::from_slice::<crate::channels::feishu::types::WsAckPayload>(&ack.payload)
            .expect("ack payload should decode");

    assert_eq!(payload.code, 200);
}
