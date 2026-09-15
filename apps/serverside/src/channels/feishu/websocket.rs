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
#[path = "../../../../../tests/unit/serverside/channels/feishu/websocket_test.rs"]
mod tests;
