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

impl StdioTransport {
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &std::collections::HashMap<String, String>,
        timeout_ms: u64,
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| McpError::SpawnFailed {
            command: command.to_string(),
            error: e.to_string(),
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
        match timeout(
            std::time::Duration::from_millis(self.timeout_ms),
            stdin.write_all(&buf),
        )
        .await
        {
            Ok(Ok(())) => {
                // `flush` is a no-op for pipes (data goes straight to the
                // kernel buffer) but call for correctness.
                if let Err(e) = stdin.flush().await {
                    // stdin is dropped here (not put back), poisoning the
                    // connection.
                    return Err(McpError::Io(e.to_string()));
                }
                *guard = Some(stdin);
                Ok(())
            }
            Ok(Err(e)) => {
                // Write error — pipe is broken. stdin is dropped, leaving
                // guard as None so future calls return TransportClosed.
                Err(McpError::Io(e.to_string()))
            }
            Err(_) => {
                // Timeout: an unknown number of bytes may have reached the
                // pipe buffer. Poison stdin (not put back) to prevent
                // subsequent writes from corrupting the framing.
                Err(McpError::Timeout {
                    timeout_ms: self.timeout_ms,
                })
            }
        }
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

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send_request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError> {
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
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let line = serde_json::to_string(&notif).map_err(|e| McpError::Protocol(e.to_string()))?;
        self.write_line(&line).await?;
        Ok(())
    }
}

// Note: no custom `Drop` — `Command::kill_on_drop(true)` ensures the child is
// killed when `Child` (held inside the `Mutex<Option<Child>>`) drops.
