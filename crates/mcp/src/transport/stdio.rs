//! stdio transport: JSON-RPC over a child process's stdin/stdout (newline
//! delimited).

use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::error::McpError;
use crate::transport::McpTransport;
use crate::types::{InboundMessage, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

type PendingMap = Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>;

pub struct StdioTransport {
    stdin: Mutex<Option<tokio::process::ChildStdin>>,
    pending: PendingMap,
    /// Holds the child process so it stays alive for the transport's lifetime.
    /// `Command::kill_on_drop(true)` kills it when the transport is dropped;
    /// the field is never explicitly read after spawn.
    #[allow(dead_code)]
    child: Mutex<Option<Child>>,
    timeout_ms: u64,
}

/// Environment variables a stdio MCP server inherits from the host process.
///
/// Everything else is dropped by [`child_env`]. Provider credentials are
/// injected into the daemon's own environment (`inject_llm_secrets_into_env`),
/// so inheriting the whole environment would hand every API key to a process
/// whose `command` may come from a workspace-scoped, agent-writable file such
/// as `.mcp.json`.
const INHERITED_ENV_NAMES: [&str; 5] = ["PATH", "HOME", "LANG", "LC_ALL", "TMPDIR"];

/// Build the environment for a stdio MCP server.
///
/// `parent` is the host environment (normally `std::env::vars()`); only
/// [`INHERITED_ENV_NAMES`] are carried over from it. The server's explicitly
/// configured `configured` variables are applied afterwards, so an operator
/// can still opt a server into a specific variable — and those entries win on
/// conflict.
fn child_env<I>(parent: I, configured: &HashMap<String, String>) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    let mut env: Vec<(String, String)> = parent
        .into_iter()
        .filter(|(key, _)| INHERITED_ENV_NAMES.contains(&key.as_str()))
        .collect();
    for (key, value) in configured {
        env.retain(|(existing, _)| existing != key);
        env.push((key.clone(), value.clone()));
    }
    env
}

impl StdioTransport {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .env_clear()
            .envs(child_env(std::env::vars(), env))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        let mut child = process_coordination::spawn_tokio_with_deadline(&mut cmd, Some(deadline))
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::TimedOut {
                    McpError::Timeout { timeout_ms }
                } else {
                    McpError::SpawnFailed {
                        command: command.to_string(),
                        error: error.to_string(),
                    }
                }
            })?;

        let stdin = child.stdin.take().ok_or_else(|| McpError::SpawnFailed {
            command: command.to_string(),
            error: "missing stdin".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| McpError::SpawnFailed {
            command: command.to_string(),
            error: "missing stdout".to_string(),
        })?;
        let stderr = child.stderr.take();

        let pending: PendingMap = Arc::new(Mutex::new(HashMap::new()));

        // Reader loop: parse each stdout line as JSON-RPC and dispatch.
        tokio::spawn(reader_loop(stdout, Arc::clone(&pending)));

        // Optional stderr pump for diagnostics. MCP servers log to stderr with
        // no parseable level, so route error-looking lines to `warn!` and the
        // rest (routine chatter, startup banners) to `debug!` to keep the
        // default log stream clean.
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if looks_like_error(trimmed) {
                        warn!(target: "mcp::stdio", line = %line, "mcp server stderr");
                    } else {
                        debug!(target: "mcp::stdio", line = %line, "mcp server stderr");
                    }
                }
            });
        }

        Ok(Self {
            stdin: Mutex::new(Some(stdin)),
            pending,
            child: Mutex::new(Some(child)),
            timeout_ms,
        })
    }

    /// Write a newline-delimited JSON-RPC message to the child's stdin,
    /// bounding the write by `timeout_ms`. Bounding the write prevents a
    /// stalled child (not reading stdin) from holding the stdin mutex and
    /// deadlocking all other senders.
    ///
    /// On timeout the connection is **poisoned**: `stdin` is taken out of
    /// the `Option` and dropped, closing the pipe's write end. A partial
    /// message may already be sitting in the kernel pipe buffer, so any
    /// further write would append to that fragment and corrupt the child's
    /// input stream (leading to JSON parse failures that never recover).
    /// Poisoning makes all subsequent calls fail fast with
    /// [`McpError::TransportClosed`] instead of silently corrupting the
    /// stream; the caller is expected to re-initialise the server.
    async fn write_line(&self, line: &str) -> Result<(), McpError> {
        let mut guard = self.stdin.lock().await;
        let mut stdin = guard.take().ok_or(McpError::TransportClosed)?;
        // Combine payload + delimiter into one buffer so only a single
        // `write_all` is issued, minimising the window for a partial write.
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        match write_buf(&mut stdin, &buf, self.timeout_ms).await {
            Ok(()) => {
                *guard = Some(stdin);
                Ok(())
            }
            // Write error or timeout: stdin is dropped, leaving guard as None
            // so future calls return TransportClosed. On timeout an unknown
            // number of bytes may already be in the pipe, so not putting
            // stdin back is what prevents framing corruption.
            Err(error) => Err(error),
        }
    }

    /// Shared request path for [`McpTransport::send_request`] and
    /// [`McpTransport::send_request_cancellable`].
    ///
    /// The frame is always fully written before the cancellation race starts,
    /// so cancelling can never leave a partial JSON-RPC line in the child's
    /// stdin (which would corrupt every later request). This also bounds when
    /// cancellation can take effect: if the child stopped reading stdin, a
    /// blocked write is only abandoned at `timeout_ms`, not immediately.
    /// Racing the write itself would require detaching it and would risk a
    /// half-written frame. On cancellation the
    /// pending entry is removed before the notification is sent: the late
    /// response then finds no receiver and is dropped by the reader loop,
    /// as MCP requires for a cancelled request.
    async fn send_request_inner(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        if cancel.is_some_and(|token| token.is_cancelled()) {
            return Err(McpError::Cancelled);
        }

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&req).map_err(|e| McpError::Protocol(e.to_string()))?;

        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        {
            let mut map = self.pending.lock().await;
            map.insert(id, tx);
        }

        if let Err(e) = self.write_line(&line).await {
            let mut map = self.pending.lock().await;
            map.remove(&id);
            return Err(e);
        }

        let wait = timeout(std::time::Duration::from_millis(self.timeout_ms), rx);
        let outcome = match cancel {
            Some(token) => tokio::select! {
                biased;
                _ = token.cancelled() => Err(McpError::Cancelled),
                result = wait => map_wait_result(result, self.timeout_ms),
            },
            None => map_wait_result(wait.await, self.timeout_ms),
        };

        let resp = match outcome {
            Ok(resp) => resp,
            Err(error) => {
                self.pending.lock().await.remove(&id);
                if matches!(error, McpError::Cancelled) {
                    self.send_cancelled_notification(id).await;
                }
                return Err(error);
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
}

/// Map the nested `timeout(oneshot)` result onto the transport error model.
fn map_wait_result(
    result: Result<
        Result<JsonRpcResponse, tokio::sync::oneshot::error::RecvError>,
        tokio::time::error::Elapsed,
    >,
    timeout_ms: u64,
) -> Result<JsonRpcResponse, McpError> {
    match result {
        Ok(Ok(resp)) => Ok(resp),
        Ok(Err(_)) => Err(McpError::Disconnected),
        Err(_) => Err(McpError::Timeout { timeout_ms }),
    }
}

/// Heuristic: treat a stderr line as an error worth surfacing at `warn!` if it
/// contains a known error marker. Everything else is the server's routine
/// chatter and goes to `debug!`.
fn looks_like_error(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("error")
        || lower.contains("fatal")
        || lower.contains("panic")
        || lower.contains("traceback")
        || lower.contains("uncaught")
        || lower.contains("exception")
}

async fn reader_loop(stdout: tokio::process::ChildStdout, pending: PendingMap) {
    let reader = BufReader::new(stdout);
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let msg: InboundMessage = match serde_json::from_str(&line) {
                    Ok(m) => m,
                    Err(error) => {
                        warn!(target: "mcp::stdio", error = %error, line = %line, "failed to parse mcp line");
                        continue;
                    }
                };
                match msg {
                    InboundMessage::Response(resp) => {
                        let id = match resp.id.as_u64() {
                            Some(id) => id,
                            None => {
                                warn!(target: "mcp::stdio", "dropping response with non-numeric string id");
                                continue;
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
            Ok(None) => break,
            Err(error) => {
                warn!(target: "mcp::stdio", error = %error, "mcp stdout read error");
                break;
            }
        }
    }
}

/// Write one framed message, bounding both the write and the flush.
///
/// The caller owns `stdin`; on error it must drop the handle instead of
/// putting it back, because a timeout can leave a partial frame in the pipe.
async fn write_buf(
    stdin: &mut tokio::process::ChildStdin,
    buf: &[u8],
    timeout_ms: u64,
) -> Result<(), McpError> {
    match timeout(
        std::time::Duration::from_millis(timeout_ms),
        stdin.write_all(buf),
    )
    .await
    {
        Ok(Ok(())) => {
            // `flush` is a no-op for pipes (data goes straight to the kernel
            // buffer) but call for correctness.
            stdin
                .flush()
                .await
                .map_err(|error| McpError::Io(error.to_string()))
        }
        Ok(Err(error)) => Err(McpError::Io(error.to_string())),
        Err(_) => Err(McpError::Timeout { timeout_ms }),
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError> {
        self.send_request_inner(id, method, params, None).await
    }

    async fn send_request_cancellable(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        self.send_request_inner(id, method, params, cancel).await
    }

    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&notif).map_err(|e| McpError::Protocol(e.to_string()))?;
        self.write_line(&line).await?;
        Ok(())
    }

    /// Best-effort cancellation notification that never queues behind an
    /// in-flight write.
    ///
    /// The default trait implementation waits up to 1s on the transport. For
    /// stdio that wait is on `stdin`: if the timeout fires after `write_all`
    /// started, dropping the future discards the `ChildStdin` taken out of the
    /// mutex and permanently poisons the connection. A best-effort
    /// notification must not cost the whole server session, so skip it when
    /// another write owns the lock, and only accept the poisoning risk once
    /// the notification write itself has started.
    async fn send_cancelled_notification(&self, id: u64) {
        let Ok(mut guard) = self.stdin.try_lock() else {
            return;
        };
        let Some(mut stdin) = guard.take() else {
            return;
        };
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: "notifications/cancelled".to_string(),
            params: Some(serde_json::json!({ "requestId": id })),
        };
        let line = match serde_json::to_string(&notif) {
            Ok(line) => line,
            Err(_) => {
                *guard = Some(stdin);
                return;
            }
        };
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        if write_buf(&mut stdin, &buf, CANCEL_NOTIFICATION_TIMEOUT_MS)
            .await
            .is_ok()
        {
            *guard = Some(stdin);
        }
    }

    async fn set_protocol_version(&self, _protocol_version: &str) {}
}

const CANCEL_NOTIFICATION_TIMEOUT_MS: u64 = 1_000;

// Note: no custom `Drop` — `Command::kill_on_drop(true)` ensures the child is
// killed when `Child` (held inside the `Mutex<Option<Child>>`) drops.

#[cfg(test)]
#[path = "../../../../tests/unit/mcp/transport/stdio_test.rs"]
mod tests;
