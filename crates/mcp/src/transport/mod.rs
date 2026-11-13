mod sse;
mod stdio;
mod streamable_http;

pub use sse::SseTransport;
pub use stdio::StdioTransport;
pub use streamable_http::StreamableHttpTransport;

use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use crate::error::McpError;

/// Upper bound on the best-effort `notifications/cancelled` write. A server
/// that stops reading stdin must not make cancellation wait forever.
const CANCELLATION_NOTIFICATION_TIMEOUT: Duration = Duration::from_secs(1);

/// 出站 HTTP transport 的"锁回 HTTP/1.1"逃生门，与
/// `crates/llm-client/src/http.rs` 的 `http1_forced` 读取同一个环境变量
/// `XIAOO_LLM_FORCE_HTTP1`、同一套取值语义：空串 / `0` / 小写 `false`
/// （仅去首尾空白，区分大小写）视为关闭，其余任意值（含 `off` / `no` /
/// `False`）一律开启。判据在这里另写一份而非复用 llm-client：mcp 是更
/// 底层的 crate，不该为三行 env 解析反向依赖 llm-client。用途：reqwest
/// 的 `http2` feature 在 workspace 层声明后经特性统一波及进程内所有
/// reqwest client，上游 MCP 网关若经 ALPN 协商到有缺陷的 h2 流式实现，
/// 现场用这一个开关把 LLM 客户端与 MCP 出站一起锁回 HTTP/1.1，无需发版。
fn with_forced_http1(builder: reqwest::ClientBuilder) -> reqwest::ClientBuilder {
    if http1_forced() {
        builder.http1_only()
    } else {
        builder
    }
}

/// [`with_forced_http1`] 的判据本体；与 llm-client 侧逐字符一致，
/// 改动取值语义时两侧必须同步。
fn http1_forced() -> bool {
    std::env::var("XIAOO_LLM_FORCE_HTTP1")
        .ok()
        .is_some_and(|value| !matches!(value.trim(), "" | "0" | "false"))
}

/// Bidirectional JSON-RPC transport. Implementations own the underlying
/// connection (child process pipes, HTTP stream) and serialise requests.
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and wait for the matching response.
    async fn send_request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError>;

    /// Send a request and abandon it as soon as `cancel` fires.
    ///
    /// The default implementation races [`send_request`](Self::send_request)
    /// against the token and, on cancellation, sends the MCP
    /// `notifications/cancelled` for `id` before returning
    /// [`McpError::Cancelled`]. Transports that own a pending-request table
    /// override this so a dropped future can be cleaned up and a partially
    /// written frame can never be left behind (see the stdio transport).
    async fn send_request_cancellable(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
        cancel: Option<&CancellationToken>,
    ) -> Result<Value, McpError> {
        let Some(cancel) = cancel else {
            return self.send_request(id, method, params).await;
        };
        if cancel.is_cancelled() {
            return Err(McpError::Cancelled);
        }
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                self.send_cancelled_notification(id).await;
                Err(McpError::Cancelled)
            }
            result = self.send_request(id, method, params) => result,
        }
    }

    /// Best-effort `notifications/cancelled` for an in-flight request.
    ///
    /// MCP makes cancellation advisory: the receiver may already have
    /// completed the request, and a notification for an unknown id is
    /// ignored. A failed or slow write must therefore not turn cancellation
    /// into a different error — both the error and the join handle are
    /// discarded here.
    async fn send_cancelled_notification(&self, id: u64) {
        let params = serde_json::json!({ "requestId": id });
        let _ = tokio::time::timeout(
            CANCELLATION_NOTIFICATION_TIMEOUT,
            self.send_notification("notifications/cancelled", Some(params)),
        )
        .await;
    }

    /// Send a fire-and-forget notification.
    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError>;

    /// Protocol version advertised in the initialize request.
    fn initialize_protocol_version(&self) -> &'static str {
        "2024-11-05"
    }

    /// Validate the version negotiated by the server.
    fn validate_negotiated_protocol_version(&self, _version: &str) -> Result<(), McpError> {
        Ok(())
    }

    /// Record the protocol version selected during initialisation. Transports
    /// that do not negotiate protocol headers deliberately ignore this.
    async fn set_protocol_version(&self, _protocol_version: &str) {}

    /// Release transport-owned resources. Legacy transports do not require an
    /// explicit close, while Streamable HTTP uses this to terminate its MCP
    /// session before the process exits.
    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/mcp/transport_test.rs"]
mod tests;
