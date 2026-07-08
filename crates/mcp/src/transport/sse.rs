//! SSE transport: JSON-RPC over HTTP with server-sent events for inbound
//! messages and POST for outbound requests. Matches the MCP "HTTP+SSE"
//! transport where the client POSTs requests to an endpoint and listens to a
//! stream of SSE events carrying responses/notifications.

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::Client as HttpClient;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;
use tracing::warn;

use crate::error::McpError;
use crate::transport::McpTransport;
use crate::types::{InboundMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

pub struct SseTransport {
    /// Base URL of the MCP HTTP server. Requests are POSTed to `{url}` (or the
    /// endpoint advertised by the server's `endpoint` SSE event). Held behind a
    /// shared mutex so the reader task can update it when the server advertises
    /// a different POST endpoint, and `send_request`/`send_notification` always
    /// read the current value.
    post_url: Arc<tokio::sync::Mutex<String>>,
    http: HttpClient,
    pending: PendingMap,
    timeout_ms: u64,
    /// Handle to the background SSE reader task. Aborted in `Drop` so the
    /// task does not outlive the transport — otherwise it would keep holding
    /// the `reqwest::Response` (the open HTTP connection) until the server
    /// closes the stream, which for long-lived SSE servers may never happen.
    reader: tokio::task::JoinHandle<()>,
}

impl SseTransport {
    pub async fn connect(url: &str, timeout_ms: u64) -> Result<Self, McpError> {
        let http = HttpClient::builder()
            .connect_timeout(std::time::Duration::from_millis(timeout_ms))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| McpError::Http(format_error(&e)))?;

        // Open the SSE stream. The server may send an `endpoint` event telling
        // us where to POST requests; otherwise we POST to the base URL.
        let response = send_timed(
            http.get(url).header("accept", "text/event-stream"),
            timeout_ms,
        )
        .await?;
        if !response.status().is_success() {
            return Err(McpError::Http(format!(
                "sse connect returned {}",
                response.status()
            )));
        }

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // For MCP, the POST endpoint may differ from the SSE URL. We start by
        // POSTing to the SSE URL and update it if the server sends an
        // `endpoint` event.
        let base_url = url.to_string();
        let post_url = Arc::new(tokio::sync::Mutex::new(url.to_string()));
        let stream = response.bytes_stream();
        let pending_for_reader = Arc::clone(&pending);
        let post_url_for_reader = Arc::clone(&post_url);

        let reader = tokio::spawn(async move {
            let mut stream = stream;
            let mut buffer: Vec<u8> = Vec::new();
            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(chunk) => {
                        buffer.extend_from_slice(&chunk);
                        normalize_crlf(&mut buffer);
                        loop {
                            // SSE events are separated by a blank line (`\n\n`
                            // after CRLF normalization). We operate on raw bytes
                            // because a chunk may end mid-codepoint, in which
                            // case `from_utf8` fails and the old code silently
                            // dropped the entire chunk. Instead, find the
                            // longest valid UTF-8 prefix and only process that;
                            // the remaining bytes (incomplete sequence) stay in
                            // the buffer until the next chunk completes them.
                            let valid_len = match std::str::from_utf8(&buffer) {
                                Ok(_) => buffer.len(),
                                Err(e) => e.valid_up_to(),
                            };
                            if valid_len == 0 {
                                break;
                            }
                            let Some(idx) = find_subsequence(&buffer[..valid_len], b"\n\n") else {
                                break;
                            };
                            let raw_event = std::str::from_utf8(&buffer[..idx])
                                .unwrap_or("")
                                .to_string();
                            buffer.drain(..idx + 2);
                            handle_sse_event(
                                &raw_event,
                                &pending_for_reader,
                                &post_url_for_reader,
                                &base_url,
                            )
                            .await;
                        }
                    }
                    Err(error) => {
                        warn!(target: "mcp::sse", error = %error, "sse stream error");
                        break;
                    }
                }
            }
        });

        Ok(Self {
            post_url,
            http,
            pending,
            timeout_ms,
            reader,
        })
    }
}

impl Drop for SseTransport {
    fn drop(&mut self) {
        // Abort the reader task so it stops polling the SSE stream and
        // releases the `reqwest::Response` (the underlying HTTP connection).
        // `JoinHandle::drop` alone does not cancel the task — without this
        // the reader would run until the server closes the stream, which for
        // long-lived SSE servers may never happen, leaking both the task and
        // the connection.
        self.reader.abort();
    }
}

/// Normalize SSE line endings to `\n` in-place: `\r\n` → `\n` and
/// standalone `\r` → `\n`. Since `\r` and `\n` are single-byte ASCII they can
/// never appear inside a multi-byte UTF-8 sequence, so byte-level replacement
/// is UTF-8-safe.
///
/// A trailing `\r` is left untouched so it can pair with a potential `\n` at the
/// start of the next chunk (a `\r\n` pair split across chunk boundaries is one
/// line terminator, not two).
fn normalize_crlf(buf: &mut Vec<u8>) {
    if buf.is_empty() {
        return;
    }
    let end = if buf.last() == Some(&b'\r') {
        buf.len() - 1
    } else {
        buf.len()
    };
    let mut read = 0;
    let mut write = 0;
    while read < end {
        if buf[read] == b'\r' {
            buf[write] = b'\n';
            write += 1;
            read += 1;
            if read < end && buf[read] == b'\n' {
                read += 1;
            }
        } else {
            if read != write {
                buf[write] = buf[read];
            }
            write += 1;
            read += 1;
        }
    }
    while read < buf.len() {
        if read != write {
            buf[write] = buf[read];
        }
        write += 1;
        read += 1;
    }
    buf.truncate(write);
}

/// Find the first occurrence of `needle` in `haystack`, comparing raw bytes.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Format a reqwest error with its full source chain for better diagnostics.
/// reqwest's `Display` only shows the error kind (e.g. "builder error") which
/// hides the underlying cause.
fn format_error(e: &reqwest::Error) -> String {
    let mut msg = e.to_string();
    let mut source = std::error::Error::source(e);
    while let Some(s) = source {
        msg.push_str(": ");
        msg.push_str(&s.to_string());
        source = s.source();
    }
    msg
}

/// Send a request, bounding the connect and response-header phase by
/// `timeout_ms`. The response body (e.g. the long-lived SSE stream) is read
/// separately by the caller and stays unbounded, so streaming responses are
/// not cut off.
async fn send_timed(
    request: reqwest::RequestBuilder,
    timeout_ms: u64,
) -> Result<reqwest::Response, McpError> {
    match timeout(std::time::Duration::from_millis(timeout_ms), request.send()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(e)) => Err(McpError::Http(format_error(&e))),
        Err(_) => Err(McpError::Timeout { timeout_ms }),
    }
}

/// Resolve an endpoint URL against the base SSE URL using RFC 3986 semantics.
/// MCP servers may advertise the POST endpoint as an absolute URL or a
/// relative path (e.g. `/messages`, `messages`). Uses `url::Url::join` for
/// correct resolution including the edge case where the base URL has a path
/// component.
fn resolve_url(base: &str, endpoint: &str) -> String {
    if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
        return endpoint.to_string();
    }
    match url::Url::parse(base) {
        Ok(base_url) => match base_url.join(endpoint) {
            Ok(resolved) => resolved.to_string(),
            Err(_) => endpoint.to_string(),
        },
        Err(_) => endpoint.to_string(),
    }
}

async fn handle_sse_event(
    raw: &str,
    pending: &PendingMap,
    post_url: &Arc<tokio::sync::Mutex<String>>,
    base_url: &str,
) {
    let mut event_type = String::new();
    let mut data = String::new();
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event_type = rest.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim());
        }
    }

    match event_type.as_str() {
        "endpoint" => {
            let new_url = data.trim().to_string();
            if !new_url.is_empty() {
                let resolved = resolve_url(base_url, &new_url);
                *post_url.lock().await = resolved;
            }
        }
        _ => {
            if data.trim().is_empty() {
                return;
            }
            let msg: InboundMessage = match serde_json::from_str(data.trim()) {
                Ok(m) => m,
                Err(error) => {
                    warn!(target: "mcp::sse", error = %error, data = %data, "failed to parse sse data");
                    return;
                }
            };
            match msg {
                InboundMessage::Response(resp) => {
                    let id = match resp.id.as_u64() {
                        Some(id) => id,
                        None => {
                            warn!(target: "mcp::sse", "dropping response with non-numeric string id");
                            return;
                        }
                    };
                    let mut map = pending.lock().await;
                    if let Some(tx) = map.remove(&id) {
                        let _ = tx.send(resp);
                    }
                }
                InboundMessage::Notification(_) => {}
            }
        }
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn send_request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError> {
        let body = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.to_string(),
            params,
        };

        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        let post_url = self.post_url.lock().await.clone();
        let post_result = send_timed(
            self.http
                .post(&post_url)
                .header("content-type", "application/json")
                .json(&body),
            self.timeout_ms,
        )
        .await
        .and_then(|response| {
            response
                .error_for_status()
                .map_err(|e| McpError::Http(format_error(&e)))
        });
        if let Err(e) = post_result {
            let mut map = self.pending.lock().await;
            map.remove(&id);
            return Err(e);
        }

        let resp = match timeout(std::time::Duration::from_millis(self.timeout_ms), rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                let mut map = self.pending.lock().await;
                map.remove(&id);
                return Err(McpError::Disconnected);
            }
            Err(_) => {
                let mut map = self.pending.lock().await;
                map.remove(&id);
                return Err(McpError::Timeout {
                    timeout_ms: self.timeout_ms,
                });
            }
        };

        if let Some(err) = resp.error {
            return Err(McpError::ServerError {
                code: err.code,
                message: err.message,
            });
        }
        resp.result
            .ok_or_else(|| McpError::Protocol("empty result".to_string()))
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let body = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let post_url = self.post_url.lock().await.clone();
        send_timed(
            self.http
                .post(&post_url)
                .header("content-type", "application/json")
                .json(&body),
            self.timeout_ms,
        )
        .await
        .and_then(|response| {
            response
                .error_for_status()
                .map_err(|e| McpError::Http(format_error(&e)))
        })?;
        Ok(())
    }
}
