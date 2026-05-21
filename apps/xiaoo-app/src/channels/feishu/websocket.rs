use crate::channels::feishu::ingress::handle_long_connection_payload;
use crate::channels::feishu::types::{
    FeishuConfig, FeishuEventTransport, WsAckPayload, WsEndpointData, WsEndpointRequest,
    WsEndpointResponse,
};
use crate::channels::{ChannelError, ChannelMessage, ChannelResult};
use futures_util::{future::BoxFuture, SinkExt, StreamExt};
use prost::Message;
use reqwest::Client;
use std::collections::HashMap;
use std::env;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::{sleep, timeout, Instant, Sleep};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};
use tracing::{debug, warn};
use url::Url;

const FRAME_TYPE_CONTROL: i32 = 0;
const FRAME_TYPE_DATA: i32 = 1;

const HEADER_TYPE: &str = "type";
const HEADER_MESSAGE_ID: &str = "message_id";
const HEADER_SUM: &str = "sum";
const HEADER_SEQ: &str = "seq";
const HEADER_TRACE_ID: &str = "trace_id";
const HEADER_BIZ_RT: &str = "biz_rt";

const MESSAGE_TYPE_EVENT: &str = "event";
const MESSAGE_TYPE_PING: &str = "ping";
const MESSAGE_TYPE_PONG: &str = "pong";

const EVENT_CHUNK_EXPIRY: Duration = Duration::from_secs(10);
const ENDPOINT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const WEBSOCKET_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_EVENT_CHUNKS: usize = 64;
const MAX_EVENT_PAYLOAD_BYTES: usize = 8 * 1024 * 1024;
const MAX_ERROR_BODY_CHARS: usize = 512;

pub type FeishuWebsocketMessageHandler =
    Arc<dyn Fn(ChannelMessage) -> BoxFuture<'static, ()> + Send + Sync>;

pub struct FeishuWebsocketService {
    config: FeishuConfig,
    http_client: Client,
    chunk_cache: EventChunkCache,
}

impl FeishuWebsocketService {
    pub fn new(config: FeishuConfig) -> ChannelResult<Self> {
        if config.event_transport != FeishuEventTransport::Websocket {
            return Err(ChannelError::Config {
                message: "feishu websocket service requires websocket transport".to_string(),
            });
        }
        config.validate().map_err(|error| ChannelError::Config {
            message: error.to_string(),
        })?;
        let http_client = Client::builder()
            .timeout(ENDPOINT_REQUEST_TIMEOUT)
            .build()
            .map_err(|error| ChannelError::Config {
                message: format!("failed to build Feishu websocket HTTP client: {error}"),
            })?;

        Ok(Self {
            config,
            http_client,
            chunk_cache: EventChunkCache::default(),
        })
    }

    pub async fn run_forever(mut self, handler: FeishuWebsocketMessageHandler) {
        loop {
            let reconnect_delay = match self.run_session(handler.clone()).await {
                Ok(delay) => {
                    warn!("feishu websocket connection closed, scheduling reconnect");
                    delay
                }
                Err(error) => {
                    warn!("feishu websocket session failed: {error}");
                    Duration::from_secs(5)
                }
            };
            sleep(reconnect_delay).await;
        }
    }

    async fn run_session(
        &mut self,
        handler: FeishuWebsocketMessageHandler,
    ) -> ChannelResult<Duration> {
        let endpoint = self.fetch_endpoint().await?;
        let service_id = parse_service_id(&endpoint.url)?;
        let reconnect_delay = reconnect_delay(&endpoint);

        let (ws_stream, _response) = timeout(
            WEBSOCKET_CONNECT_TIMEOUT,
            connect_async(endpoint.url.as_str()),
        )
        .await
        .map_err(|_| ChannelError::Transport {
            message: format!(
                "timed out connecting Feishu websocket endpoint after {}s",
                WEBSOCKET_CONNECT_TIMEOUT.as_secs()
            ),
        })?
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to connect feishu websocket: {error}"),
        })?;
        let (mut writer, mut reader) = ws_stream.split();

        let mut ping_sleep = boxed_sleep(ping_interval(&endpoint));

        loop {
            tokio::select! {
                _ = &mut ping_sleep => {
                    send_frame(&mut writer, build_ping_frame(service_id)).await?;
                    ping_sleep.as_mut().reset(Instant::now() + ping_interval(&endpoint));
                }
                message = reader.next() => {
                    match message {
                        Some(Ok(WsMessage::Binary(binary))) => {
                            if let Some(next_ping_interval) = self
                                .handle_binary_frame(&mut writer, binary.as_ref(), handler.clone())
                                .await?
                            {
                                let duration = Duration::from_secs(next_ping_interval.max(1));
                                ping_sleep.as_mut().reset(Instant::now() + duration);
                            }
                        }
                        Some(Ok(WsMessage::Close(_))) => return Ok(reconnect_delay),
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            return Err(ChannelError::Transport {
                                message: format!("feishu websocket read failed: {error}"),
                            });
                        }
                        None => return Ok(reconnect_delay),
                    }
                }
            }
        }
    }

    async fn handle_binary_frame(
        &mut self,
        writer: &mut (impl futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error>
                  + Unpin),
        payload: &[u8],
        handler: FeishuWebsocketMessageHandler,
    ) -> ChannelResult<Option<u64>> {
        let frame = ProtoFrame::decode(payload).map_err(|error| ChannelError::Transport {
            message: format!("failed to decode feishu websocket frame: {error}"),
        })?;

        match frame.method {
            FRAME_TYPE_CONTROL => self.handle_control_frame(frame),
            FRAME_TYPE_DATA => self.handle_data_frame(writer, frame, handler).await,
            other => Err(ChannelError::Transport {
                message: format!("unsupported feishu websocket frame type `{other}`"),
            }),
        }
    }

    fn handle_control_frame(&self, frame: ProtoFrame) -> ChannelResult<Option<u64>> {
        let message_type = frame.header(HEADER_TYPE);
        if message_type == Some(MESSAGE_TYPE_PONG) && !frame.payload.is_empty() {
            let value =
                serde_json::from_slice::<serde_json::Value>(&frame.payload).map_err(|error| {
                    ChannelError::Transport {
                        message: format!("failed to parse feishu websocket pong payload: {error}"),
                    }
                })?;
            let ping_interval_secs = value
                .get("PingInterval")
                .and_then(|value| value.as_u64())
                .unwrap_or(120);
            return Ok(Some(ping_interval_secs));
        }

        Ok(None)
    }

    async fn handle_data_frame(
        &mut self,
        writer: &mut (impl futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error>
                  + Unpin),
        frame: ProtoFrame,
        handler: FeishuWebsocketMessageHandler,
    ) -> ChannelResult<Option<u64>> {
        if frame.header(HEADER_TYPE) != Some(MESSAGE_TYPE_EVENT) {
            return Ok(None);
        }

        let Some(message_id) = frame.header(HEADER_MESSAGE_ID).map(ToString::to_string) else {
            return Err(ChannelError::Transport {
                message: "missing feishu websocket message_id header".to_string(),
            });
        };
        let sum = parse_header_usize(&frame, HEADER_SUM)?;
        let seq = parse_header_usize(&frame, HEADER_SEQ)?;
        let trace_id = frame
            .header(HEADER_TRACE_ID)
            .map(ToString::to_string)
            .unwrap_or_default();

        let Some(merged_payload) = self.chunk_cache.push(
            message_id.clone(),
            trace_id,
            sum,
            seq,
            frame.payload.clone(),
        )?
        else {
            return Ok(None);
        };

        let started_at = Instant::now();
        let ack_code = match handle_long_connection_payload(&self.config, &merged_payload) {
            Ok(message) => {
                let ack = build_ack_frame(&frame, 200, started_at.elapsed())?;
                send_frame(writer, ack).await?;
                if let Some(message) = message {
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        (handler)(message).await;
                    });
                }
                debug!("processed feishu websocket event: message_id={message_id}");
                200
            }
            Err(error) => {
                let ack = build_ack_frame(&frame, 500, started_at.elapsed())?;
                send_frame(writer, ack).await?;
                warn!(
                    "failed to parse feishu websocket event: message_id={} error={}",
                    message_id, error
                );
                500
            }
        };

        debug!(
            "acknowledged feishu websocket event: message_id={} code={}",
            message_id, ack_code
        );

        Ok(None)
    }

    async fn fetch_endpoint(&self) -> ChannelResult<WsEndpointData> {
        let app_secret =
            env::var(self.config.app_secret_env.trim()).map_err(|error| ChannelError::Config {
                message: format!(
                    "failed to read Feishu app secret from env `{}`: {error}",
                    self.config.app_secret_env.trim()
                ),
            })?;

        let response = self
            .http_client
            .post(format!("{}/callback/ws/endpoint", self.config.base_url()))
            .json(&WsEndpointRequest {
                app_id: self.config.app_id.as_str(),
                app_secret: app_secret.as_str(),
            })
            .send()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("failed to fetch Feishu websocket endpoint: {error}"),
            })?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_else(|error| {
                format!("failed to read Feishu websocket endpoint error body: {error}")
            });
            return Err(ChannelError::Transport {
                message: format!(
                    "Feishu websocket endpoint request failed with status {status}: {}",
                    truncate_chars(&body, MAX_ERROR_BODY_CHARS)
                ),
            });
        }

        let endpoint = response
            .json::<WsEndpointResponse>()
            .await
            .map_err(|error| ChannelError::Transport {
                message: format!("failed to decode Feishu websocket endpoint response: {error}"),
            })?;

        if endpoint.code != 0 {
            return Err(ChannelError::Transport {
                message: format!(
                    "Feishu websocket endpoint request failed: code={} msg={}",
                    endpoint.code,
                    endpoint.msg.unwrap_or_else(|| "unknown error".to_string())
                ),
            });
        }

        endpoint.data.ok_or_else(|| ChannelError::Transport {
            message: "Feishu websocket endpoint response missing data".to_string(),
        })
    }
}

fn ping_interval(endpoint: &WsEndpointData) -> Duration {
    Duration::from_secs(endpoint.client_config.ping_interval_secs.max(1))
}

fn reconnect_delay(endpoint: &WsEndpointData) -> Duration {
    let base = endpoint.client_config.reconnect_interval_secs.max(1);
    let nonce = endpoint.client_config.reconnect_nonce_secs;
    Duration::from_secs(base + nonce)
}

fn parse_service_id(endpoint_url: &str) -> ChannelResult<i32> {
    let url = Url::parse(endpoint_url).map_err(|error| ChannelError::Config {
        message: format!("invalid Feishu websocket endpoint URL: {error}"),
    })?;
    if !matches!(url.scheme(), "ws" | "wss") {
        return Err(ChannelError::Config {
            message: format!(
                "invalid Feishu websocket endpoint URL scheme `{}`",
                url.scheme()
            ),
        });
    }

    let Some(service_id) = url
        .query_pairs()
        .find_map(|(key, value)| (key == "service_id").then(|| value.to_string()))
    else {
        return Err(ChannelError::Config {
            message: "Feishu websocket endpoint missing service_id".to_string(),
        });
    };

    service_id
        .parse::<i32>()
        .map_err(|error| ChannelError::Config {
            message: format!("invalid Feishu websocket service_id `{service_id}`: {error}"),
        })
}

fn parse_header_usize(frame: &ProtoFrame, key: &str) -> ChannelResult<usize> {
    let Some(value) = frame.header(key) else {
        return Err(ChannelError::Transport {
            message: format!("missing Feishu websocket header `{key}`"),
        });
    };
    value
        .parse::<usize>()
        .map_err(|error| ChannelError::Transport {
            message: format!("invalid Feishu websocket header `{key}` value `{value}`: {error}"),
        })
}

fn build_ping_frame(service_id: i32) -> ProtoFrame {
    ProtoFrame {
        seq_id: 0,
        log_id: 0,
        service: service_id,
        method: FRAME_TYPE_CONTROL,
        headers: vec![ProtoHeader::new(HEADER_TYPE, MESSAGE_TYPE_PING)],
        payload_encoding: String::new(),
        payload_type: String::new(),
        payload: Vec::new(),
        log_id_new: String::new(),
    }
}

fn build_ack_frame(frame: &ProtoFrame, code: u16, elapsed: Duration) -> ChannelResult<ProtoFrame> {
    let mut headers = frame.headers.clone();
    headers.push(ProtoHeader::new(
        HEADER_BIZ_RT,
        elapsed.as_millis().to_string(),
    ));

    let payload =
        serde_json::to_vec(&WsAckPayload { code }).map_err(|error| ChannelError::Transport {
            message: format!("failed to serialize Feishu websocket ack payload: {error}"),
        })?;

    Ok(ProtoFrame {
        seq_id: frame.seq_id,
        log_id: frame.log_id,
        service: frame.service,
        method: frame.method,
        headers,
        payload_encoding: frame.payload_encoding.clone(),
        payload_type: frame.payload_type.clone(),
        payload,
        log_id_new: frame.log_id_new.clone(),
    })
}

async fn send_frame(
    writer: &mut (impl futures_util::Sink<WsMessage, Error = tokio_tungstenite::tungstenite::Error>
              + Unpin),
    frame: ProtoFrame,
) -> ChannelResult<()> {
    writer
        .send(WsMessage::Binary(frame.encode_to_vec().into()))
        .await
        .map_err(|error| ChannelError::Transport {
            message: format!("failed to write Feishu websocket frame: {error}"),
        })
}

fn boxed_sleep(duration: Duration) -> std::pin::Pin<Box<Sleep>> {
    Box::pin(tokio::time::sleep(duration))
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[derive(Debug, Default)]
struct EventChunkCache {
    entries: HashMap<String, CachedChunks>,
}

impl EventChunkCache {
    fn push(
        &mut self,
        message_id: String,
        trace_id: String,
        sum: usize,
        seq: usize,
        payload: Vec<u8>,
    ) -> ChannelResult<Option<Vec<u8>>> {
        self.clear_expired();
        if sum == 0 {
            return Err(ChannelError::Transport {
                message: format!("invalid chunk count for message `{message_id}`"),
            });
        }
        if sum > MAX_EVENT_CHUNKS {
            return Err(ChannelError::Transport {
                message: format!(
                    "chunk count `{sum}` for message `{message_id}` exceeds maximum `{MAX_EVENT_CHUNKS}`"
                ),
            });
        }
        if seq >= sum {
            return Err(ChannelError::Transport {
                message: format!(
                    "invalid chunk index `{seq}` for message `{message_id}` with sum `{sum}`"
                ),
            });
        }
        if payload.len() > MAX_EVENT_PAYLOAD_BYTES {
            return Err(ChannelError::Transport {
                message: format!(
                    "chunk payload for message `{message_id}` exceeds maximum `{MAX_EVENT_PAYLOAD_BYTES}` bytes"
                ),
            });
        }

        let entry = self
            .entries
            .entry(message_id.clone())
            .or_insert_with(|| CachedChunks::new(trace_id.clone(), sum));
        if entry.trace_id != trace_id {
            return Err(ChannelError::Transport {
                message: format!("mismatched trace_id for message `{message_id}`"),
            });
        }
        if entry.parts.len() != sum {
            return Err(ChannelError::Transport {
                message: format!(
                    "mismatched chunk count for message `{message_id}`: expected {} got {sum}",
                    entry.parts.len()
                ),
            });
        }
        if entry.parts[seq].is_some() {
            return Err(ChannelError::Transport {
                message: format!("duplicate chunk index `{seq}` for message `{message_id}`"),
            });
        }
        entry.total_bytes = entry
            .total_bytes
            .checked_add(payload.len())
            .ok_or_else(|| ChannelError::Transport {
                message: format!("chunk payload size overflow for message `{message_id}`"),
            })?;
        if entry.total_bytes > MAX_EVENT_PAYLOAD_BYTES {
            return Err(ChannelError::Transport {
                message: format!(
                    "merged payload for message `{message_id}` exceeds maximum `{MAX_EVENT_PAYLOAD_BYTES}` bytes"
                ),
            });
        }
        entry.parts[seq] = Some(payload);

        if entry.parts.iter().all(Option::is_some) {
            let Some(entry) = self.entries.remove(&message_id) else {
                return Err(ChannelError::Transport {
                    message: format!(
                        "missing completed chunk cache entry for message `{message_id}`"
                    ),
                });
            };
            let merged =
                entry
                    .parts
                    .into_iter()
                    .flatten()
                    .fold(Vec::new(), |mut combined, part| {
                        combined.extend_from_slice(&part);
                        combined
                    });
            return Ok(Some(merged));
        }

        Ok(None)
    }

    fn clear_expired(&mut self) {
        let now = Instant::now();
        self.entries
            .retain(|_, entry| now.duration_since(entry.created_at) <= EVENT_CHUNK_EXPIRY);
    }
}

#[derive(Debug)]
struct CachedChunks {
    trace_id: String,
    created_at: Instant,
    parts: Vec<Option<Vec<u8>>>,
    total_bytes: usize,
}

impl CachedChunks {
    fn new(trace_id: String, sum: usize) -> Self {
        Self {
            trace_id,
            created_at: Instant::now(),
            parts: vec![None; sum],
            total_bytes: 0,
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct ProtoHeader {
    #[prost(string, tag = "1")]
    key: String,
    #[prost(string, tag = "2")]
    value: String,
}

impl ProtoHeader {
    fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
        }
    }
}

#[derive(Clone, PartialEq, Message)]
struct ProtoFrame {
    #[prost(uint64, tag = "1")]
    seq_id: u64,
    #[prost(uint64, tag = "2")]
    log_id: u64,
    #[prost(int32, tag = "3")]
    service: i32,
    #[prost(int32, tag = "4")]
    method: i32,
    #[prost(message, repeated, tag = "5")]
    headers: Vec<ProtoHeader>,
    #[prost(string, tag = "6")]
    payload_encoding: String,
    #[prost(string, tag = "7")]
    payload_type: String,
    #[prost(bytes = "vec", tag = "8")]
    payload: Vec<u8>,
    #[prost(string, tag = "9")]
    log_id_new: String,
}

impl ProtoFrame {
    fn header(&self, key: &str) -> Option<&str> {
        self.headers
            .iter()
            .find_map(|header| (header.key == key).then_some(header.value.as_str()))
    }
}

#[cfg(test)]
mod tests {
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
}
