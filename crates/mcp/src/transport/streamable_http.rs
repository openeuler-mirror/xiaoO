//! Streamable HTTP transport for MCP 2025-11-25.

use async_trait::async_trait;
use futures_util::StreamExt;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use reqwest::{Client as HttpClient, RequestBuilder, Response};
use serde_json::Value;
use std::sync::Mutex as StdMutex;
use tokio::sync::{Mutex, Notify};
use tokio::time::{sleep, timeout, Instant};

use crate::config::{validate_fixed_headers, McpServerConfig};
use crate::error::McpError;
use crate::transport::McpTransport;
use crate::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

const MCP_SESSION_ID: &str = "mcp-session-id";
const MCP_PROTOCOL_VERSION: &str = "mcp-protocol-version";
const X_AGENT_ID: &str = "x-agent-id";
const MAX_CANCELLATION_TIMEOUT_MS: u64 = 100;
const MAX_INITIALIZE_RATE_LIMIT_RETRIES: u8 = 1;
const DEFAULT_INITIALIZE_RATE_LIMIT_RETRY_DELAY: std::time::Duration =
    std::time::Duration::from_millis(100);

/// MCP's request/response HTTP transport. It keeps only configuration names
/// and negotiated state; bearer values are read from the environment for each
/// request and are never retained.
pub struct StreamableHttpTransport {
    url: String,
    http: HttpClient,
    timeout_ms: u64,
    bearer_token_env: Option<String>,
    agent_id: Option<String>,
    headers: HeaderMap,
    session: StdMutex<SessionState>,
    session_recovery: Mutex<()>,
    close_gate: Mutex<()>,
    active_requests_changed: Notify,
    recovery_changed: Notify,
    initialize_params: Mutex<Option<Value>>,
    initialize_id: Mutex<Option<u64>>,
}

#[derive(Clone)]
struct SessionState {
    session_id: Option<HeaderValue>,
    protocol_version: Option<HeaderValue>,
    generation: u64,
    recovery: RecoveryState,
    closing: bool,
    active_requests: usize,
}

impl Default for SessionState {
    fn default() -> Self {
        Self {
            session_id: None,
            protocol_version: None,
            generation: 0,
            recovery: RecoveryState::Idle,
            closing: false,
            active_requests: 0,
        }
    }
}

#[derive(Clone)]
enum RecoveryState {
    Idle,
    InProgress { generation: u64 },
    Failed { generation: u64, error: McpError },
}

#[derive(Clone)]
struct SessionSnapshot {
    session_id: Option<HeaderValue>,
    protocol_version: Option<HeaderValue>,
    generation: u64,
}

struct SentResponse {
    response: Response,
    session: SessionSnapshot,
}

struct ParsedResponse {
    response: JsonRpcResponse,
    initialize_session_id: Option<HeaderValue>,
}

struct RecoveryLease<'a> {
    transport: &'a StreamableHttpTransport,
    generation: u64,
    finished: bool,
}

struct RequestLease<'a> {
    transport: &'a StreamableHttpTransport,
}

impl Drop for RequestLease<'_> {
    fn drop(&mut self) {
        let mut session = self
            .transport
            .session
            .lock()
            .expect("session mutex poisoned");
        session.active_requests = session.active_requests.saturating_sub(1);
        if session.active_requests == 0 {
            self.transport.active_requests_changed.notify_waiters();
        }
    }
}

impl RecoveryLease<'_> {
    fn finish(&mut self) {
        self.finished = true;
    }
}

impl Drop for RecoveryLease<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut session = self
            .transport
            .session
            .lock()
            .expect("session mutex poisoned");
        if session.generation == self.generation
            && matches!(session.recovery, RecoveryState::InProgress { .. })
        {
            session.recovery = RecoveryState::Idle;
            self.transport.recovery_changed.notify_waiters();
        }
    }
}

enum RequestAttemptError {
    Mcp(McpError),
    RateLimited {
        retry_after: std::time::Duration,
        error: McpError,
    },
    SessionNotFound {
        session: SessionSnapshot,
        error: McpError,
    },
}

impl From<McpError> for RequestAttemptError {
    fn from(error: McpError) -> Self {
        Self::Mcp(error)
    }
}

impl StreamableHttpTransport {
    pub async fn connect(config: &McpServerConfig) -> Result<Self, McpError> {
        let url = config.url.clone().ok_or_else(|| {
            McpError::Protocol("streamable_http transport requires `url`".to_string())
        })?;
        validate_fixed_headers(&config.headers)
            .map_err(|message| McpError::Protocol(format!("invalid fixed header: {message}")))?;
        let mut headers = HeaderMap::new();
        for (name, value) in &config.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| McpError::Protocol(format!("invalid HTTP header name: {e}")))?;
            let value = HeaderValue::from_str(value)
                .map_err(|e| McpError::Protocol(format!("invalid HTTP header value: {e}")))?;
            headers.insert(name, value);
        }
        let http = HttpClient::builder()
            .connect_timeout(std::time::Duration::from_millis(config.timeout_ms))
            .tcp_keepalive(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| McpError::Http(format_error(&e)))?;

        Ok(Self {
            url,
            http,
            timeout_ms: config.timeout_ms,
            bearer_token_env: config.bearer_token_env.clone(),
            agent_id: config.agent_id.clone(),
            headers,
            session: StdMutex::new(SessionState::default()),
            session_recovery: Mutex::new(()),
            close_gate: Mutex::new(()),
            active_requests_changed: Notify::new(),
            recovery_changed: Notify::new(),
            initialize_params: Mutex::new(None),
            initialize_id: Mutex::new(None),
        })
    }

    async fn post(
        &self,
        body: &impl serde::Serialize,
        deadline: Instant,
    ) -> Result<SentResponse, McpError> {
        let session = self.session_snapshot(deadline).await?;
        self.post_with_session(body, deadline, session).await
    }

    fn begin_request(&self) -> Result<RequestLease<'_>, McpError> {
        let mut session = self.session.lock().expect("session mutex poisoned");
        if session.closing {
            return Err(McpError::Disconnected);
        }
        session.active_requests += 1;
        Ok(RequestLease { transport: self })
    }

    async fn wait_for_active_requests(&self) {
        loop {
            let changed = self.active_requests_changed.notified();
            if self
                .session
                .lock()
                .expect("session mutex poisoned")
                .active_requests
                == 0
            {
                return;
            }
            changed.await;
        }
    }

    async fn post_with_session(
        &self,
        body: &impl serde::Serialize,
        deadline: Instant,
        session: SessionSnapshot,
    ) -> Result<SentResponse, McpError> {
        let mut request = self
            .http
            .post(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(CONTENT_TYPE, "application/json")
            .json(body);
        if let Some(session_id) = &session.session_id {
            request = request.header(MCP_SESSION_ID, session_id);
        }
        if let Some(protocol_version) = &session.protocol_version {
            request = request.header(MCP_PROTOCOL_VERSION, protocol_version);
        }
        if let Some(agent_id) = &self.agent_id {
            request = request.header(X_AGENT_ID, agent_id);
        }
        if let Some(env_var) = &self.bearer_token_env {
            let token = std::env::var(env_var).map_err(|_| McpError::BearerTokenUnavailable {
                env_var: env_var.clone(),
            })?;
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = send_timed(request, deadline, self.timeout_ms).await?;
        Ok(SentResponse { response, session })
    }

    async fn session_snapshot(&self, deadline: Instant) -> Result<SessionSnapshot, McpError> {
        loop {
            let recovery_changed = self.recovery_changed.notified();
            let (recovery, snapshot) = {
                let session = self.session.lock().expect("session mutex poisoned");
                (
                    session.recovery.clone(),
                    SessionSnapshot {
                        session_id: session.session_id.clone(),
                        protocol_version: session.protocol_version.clone(),
                        generation: session.generation,
                    },
                )
            };
            match recovery {
                RecoveryState::Idle => {
                    return Ok(snapshot);
                }
                RecoveryState::Failed { generation, error }
                    if generation == snapshot.generation =>
                {
                    return Err(error.clone());
                }
                RecoveryState::InProgress { generation } if generation == snapshot.generation => {
                    timeout(remaining(deadline), recovery_changed)
                        .await
                        .map_err(|_| McpError::Timeout {
                            timeout_ms: self.timeout_ms,
                        })?;
                }
                RecoveryState::Failed { .. } | RecoveryState::InProgress { .. } => {
                    return Err(McpError::Protocol(
                        "session recovery state generation mismatch".to_string(),
                    ));
                }
            }
        }
    }

    async fn install_initialized_state(
        &self,
        session_id: Option<HeaderValue>,
        protocol_version: &str,
    ) {
        let mut session = self.session.lock().expect("session mutex poisoned");
        session.session_id = session_id;
        session.protocol_version = HeaderValue::from_str(protocol_version).ok();
        session.generation = session.generation.wrapping_add(1);
        session.recovery = RecoveryState::Idle;
        self.recovery_changed.notify_waiters();
    }

    async fn recover_session(
        &self,
        stale_session: &SessionSnapshot,
        deadline: Instant,
    ) -> Result<(), McpError> {
        let _recovery = timeout(remaining(deadline), self.session_recovery.lock())
            .await
            .map_err(|_| McpError::Timeout {
                timeout_ms: self.timeout_ms,
            })?;
        {
            let mut current = self.session.lock().expect("session mutex poisoned");
            if current.generation != stale_session.generation
                || current.session_id != stale_session.session_id
            {
                return Ok(());
            }
            match &current.recovery {
                RecoveryState::Failed { generation, error }
                    if *generation == stale_session.generation =>
                {
                    return Err(error.clone());
                }
                RecoveryState::Idle => {
                    current.recovery = RecoveryState::InProgress {
                        generation: stale_session.generation,
                    };
                }
                RecoveryState::InProgress { .. } | RecoveryState::Failed { .. } => {
                    return Err(McpError::Protocol(
                        "session recovery state generation mismatch".to_string(),
                    ));
                }
            }
        }
        let mut lease = RecoveryLease {
            transport: self,
            generation: stale_session.generation,
            finished: false,
        };
        let recovered = self.reinitialize(stale_session, deadline).await;
        let mut current = self.session.lock().expect("session mutex poisoned");
        match recovered {
            Ok(recovered) => {
                current.session_id = recovered.session_id;
                current.protocol_version = recovered.protocol_version;
                current.generation = current.generation.wrapping_add(1);
                current.recovery = RecoveryState::Idle;
                self.recovery_changed.notify_waiters();
                lease.finish();
                Ok(())
            }
            Err(error) => {
                current.recovery = RecoveryState::Failed {
                    generation: stale_session.generation,
                    error: error.clone(),
                };
                self.recovery_changed.notify_waiters();
                lease.finish();
                Err(error)
            }
        }
    }

    async fn reinitialize(
        &self,
        stale_session: &SessionSnapshot,
        deadline: Instant,
    ) -> Result<SessionSnapshot, McpError> {
        let params = self.initialize_params.lock().await.clone().ok_or_else(|| {
            McpError::Protocol("missing initialize parameters for session recovery".to_string())
        })?;
        let id = self.initialize_id.lock().await.ok_or_else(|| {
            McpError::Protocol("missing initialize id for session recovery".to_string())
        })?;
        let body = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: "initialize".to_string(),
            params: Some(params),
        };
        let parsed = self
            .request_initialize_with_rate_limit_retry(
                &body,
                id,
                deadline,
                Some(SessionSnapshot {
                    session_id: None,
                    protocol_version: None,
                    generation: stale_session.generation,
                }),
            )
            .await
            .map_err(request_attempt_into_mcp)?;
        if let Some(error) = parsed.response.error {
            return Err(McpError::ServerError {
                code: error.code,
                message: error.message,
            });
        }
        let result = parsed.response.result.ok_or_else(|| {
            McpError::Protocol("empty initialize result during session recovery".to_string())
        })?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                McpError::HandshakeFailed("initialize result omitted protocolVersion".to_string())
            })?;
        self.validate_negotiated_protocol_version(version)?;
        let recovered = SessionSnapshot {
            session_id: parsed.initialize_session_id,
            protocol_version: HeaderValue::from_str(version).ok(),
            generation: stale_session.generation.wrapping_add(1),
        };
        self.send_notification_with_session_until(
            "notifications/initialized",
            None,
            deadline,
            recovered.clone(),
        )
        .await?;
        Ok(recovered)
    }

    async fn response_for_id(
        &self,
        sent: SentResponse,
        id: u64,
        deadline: Instant,
    ) -> Result<ParsedResponse, RequestAttemptError> {
        let SentResponse { response, session } = sent;
        if response.status().is_redirection() {
            return Err(McpError::Http(format!(
                "mcp endpoint returned redirect status {}",
                response.status()
            ))
            .into());
        }
        if response.status() == reqwest::StatusCode::NOT_FOUND && session.session_id.is_some() {
            let error = response.error_for_status().unwrap_err();
            return Err(RequestAttemptError::SessionNotFound {
                session,
                error: McpError::Http(format_error(&error)),
            });
        }
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = retry_after_delay(response.headers());
            let error = response.error_for_status().unwrap_err();
            return Err(RequestAttemptError::RateLimited {
                retry_after,
                error: McpError::Http(format_error(&error)),
            });
        }
        let initialize_session_id = validate_response_session_id(response.headers())?;
        let mut reconnect_session = session;
        if let Some(session_id) = &initialize_session_id {
            reconnect_session.session_id = Some(session_id.clone());
        }
        let response = response
            .error_for_status()
            .map_err(|e| McpError::Http(format_error(&e)))?;
        let is_sse = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("text/event-stream"));
        let parsed = if is_sse {
            timeout(
                remaining(deadline),
                self.read_sse_response(response, id, deadline, reconnect_session),
            )
            .await
            .map_err(|_| McpError::Timeout {
                timeout_ms: self.timeout_ms,
            })??
        } else {
            timeout(remaining(deadline), response.json::<JsonRpcResponse>())
                .await
                .map_err(|_| McpError::Timeout {
                    timeout_ms: self.timeout_ms,
                })?
                .map_err(|e| McpError::Protocol(format!("parse JSON-RPC response: {e}")))?
        };
        if parsed.id.as_u64() != Some(id) {
            return Err(McpError::Protocol(format!(
                "response id {:?} does not match request id {id}",
                parsed.id
            ))
            .into());
        }
        Ok(ParsedResponse {
            response: parsed,
            initialize_session_id,
        })
    }

    async fn get_sse_with_session(
        &self,
        last_event_id: &str,
        deadline: Instant,
        session: SessionSnapshot,
    ) -> Result<SentResponse, McpError> {
        let mut request = self
            .http
            .get(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "text/event-stream")
            .header("last-event-id", last_event_id);
        if let Some(session_id) = &session.session_id {
            request = request.header(MCP_SESSION_ID, session_id);
        }
        if let Some(protocol_version) = &session.protocol_version {
            request = request.header(MCP_PROTOCOL_VERSION, protocol_version);
        }
        if let Some(agent_id) = &self.agent_id {
            request = request.header(X_AGENT_ID, agent_id);
        }
        if let Some(env_var) = &self.bearer_token_env {
            let token = std::env::var(env_var).map_err(|_| McpError::BearerTokenUnavailable {
                env_var: env_var.clone(),
            })?;
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = send_timed(request, deadline, self.timeout_ms).await?;
        Ok(SentResponse { response, session })
    }

    async fn delete_session(&self, deadline: Instant) -> Result<(), McpError> {
        let session = {
            let session = self.session.lock().expect("session mutex poisoned");
            SessionSnapshot {
                session_id: session.session_id.clone(),
                protocol_version: session.protocol_version.clone(),
                generation: session.generation,
            }
        };
        let Some(session_id) = session.session_id.as_ref() else {
            return Ok(());
        };

        let mut request = self
            .http
            .delete(&self.url)
            .headers(self.headers.clone())
            .header(ACCEPT, "application/json, text/event-stream")
            .header(MCP_SESSION_ID, session_id);
        if let Some(protocol_version) = &session.protocol_version {
            request = request.header(MCP_PROTOCOL_VERSION, protocol_version);
        }
        if let Some(agent_id) = &self.agent_id {
            request = request.header(X_AGENT_ID, agent_id);
        }
        if let Some(env_var) = &self.bearer_token_env {
            let token = std::env::var(env_var).map_err(|_| McpError::BearerTokenUnavailable {
                env_var: env_var.clone(),
            })?;
            request = request.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        let response = send_timed(request, deadline, self.timeout_ms).await?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(McpError::Http(format!(
                "mcp session termination returned {}",
                response.status()
            )));
        }
        let mut current = self.session.lock().expect("session mutex poisoned");
        if current.generation == session.generation && current.session_id == session.session_id {
            current.session_id = None;
            current.protocol_version = None;
            current.generation = current.generation.wrapping_add(1);
            current.recovery = RecoveryState::Idle;
            self.recovery_changed.notify_waiters();
        }
        Ok(())
    }

    async fn close_session(&self) -> Result<(), McpError> {
        let _close = self.close_gate.lock().await;
        {
            let mut session = self.session.lock().expect("session mutex poisoned");
            if session.closing && session.session_id.is_none() {
                return Ok(());
            }
            session.closing = true;
        }
        self.wait_for_active_requests().await;
        self.delete_session(overall_deadline(self.timeout_ms)).await
    }

    async fn read_sse_response(
        &self,
        response: Response,
        expected_id: u64,
        deadline: Instant,
        reconnect_session: SessionSnapshot,
    ) -> Result<JsonRpcResponse, RequestAttemptError> {
        let mut stream = response.bytes_stream();
        let mut buffer = Vec::new();
        let mut mismatched_id = None;
        let mut reconnected = false;
        let mut last_event_id = None;
        let mut retry_delay = std::time::Duration::ZERO;
        loop {
            while let Some(chunk) = stream.next().await {
                buffer.extend_from_slice(&chunk.map_err(|e| McpError::Http(format_error(&e)))?);
                normalize_crlf(&mut buffer);
                loop {
                    let valid_len = match std::str::from_utf8(&buffer) {
                        Ok(_) => buffer.len(),
                        Err(error) => error.valid_up_to(),
                    };
                    let Some(event_end) = find_subsequence(&buffer[..valid_len], b"\n\n") else {
                        break;
                    };
                    let raw_event = std::str::from_utf8(&buffer[..event_end])
                        .map_err(|e| McpError::Protocol(format!("invalid UTF-8 SSE event: {e}")))?;
                    let event = parse_sse_event(raw_event)?;
                    buffer.drain(..event_end + 2);
                    if let Some(event_id) = event.id {
                        match event_id {
                            SseEventId::Set(event_id) => last_event_id = Some(event_id),
                            SseEventId::Reset => last_event_id = None,
                        }
                    }
                    if let Some(delay) = event.retry {
                        retry_delay = delay;
                    }
                    if let Some(response) = event.response {
                        if response.id.as_u64() == Some(expected_id) {
                            return Ok(response);
                        }
                        mismatched_id = Some(response.id);
                    }
                }
            }
            if reconnected {
                break;
            }
            let Some(last_event_id) = last_event_id.as_deref() else {
                break;
            };
            tokio::time::sleep(retry_delay).await;
            let sent = self
                .get_sse_with_session(last_event_id, deadline, reconnect_session.clone())
                .await?;
            if sent.response.status().is_redirection() {
                return Err(McpError::Http(format!(
                    "mcp endpoint returned redirect status {}",
                    sent.response.status()
                ))
                .into());
            }
            if sent.response.status() == reqwest::StatusCode::NOT_FOUND
                && sent.session.session_id.is_some()
            {
                let error = sent.response.error_for_status().unwrap_err();
                return Err(RequestAttemptError::SessionNotFound {
                    session: sent.session,
                    error: McpError::Http(format_error(&error)),
                });
            }
            validate_response_session_id(sent.response.headers())?;
            let response = sent
                .response
                .error_for_status()
                .map_err(|e| McpError::Http(format_error(&e)))?;
            validate_sse_content_type(response.headers())?;
            stream = response.bytes_stream();
            buffer.clear();
            reconnected = true;
        }
        if let Some(id) = mismatched_id {
            return Err(McpError::Protocol(format!(
                "response id {:?} does not match request id {expected_id}",
                id
            ))
            .into());
        }
        Err(McpError::Disconnected.into())
    }
}

impl StreamableHttpTransport {
    async fn request_once(
        &self,
        body: &JsonRpcRequest,
        id: u64,
        deadline: Instant,
    ) -> Result<ParsedResponse, RequestAttemptError> {
        let response = self.post(body, deadline).await?;
        self.response_for_id(response, id, deadline).await
    }

    async fn request_with_session_once(
        &self,
        body: &JsonRpcRequest,
        id: u64,
        deadline: Instant,
        session: SessionSnapshot,
    ) -> Result<ParsedResponse, RequestAttemptError> {
        let response = self.post_with_session(body, deadline, session).await?;
        self.response_for_id(response, id, deadline).await
    }

    /// An initialize request is safe to retry once after a server-provided
    /// rate limit. We deliberately do not apply this to arbitrary MCP tool
    /// calls because a server may have performed a side effect before it
    /// returned 429.
    async fn request_initialize_with_rate_limit_retry(
        &self,
        body: &JsonRpcRequest,
        id: u64,
        deadline: Instant,
        session: Option<SessionSnapshot>,
    ) -> Result<ParsedResponse, RequestAttemptError> {
        let mut retries = 0;
        loop {
            let attempt = match &session {
                Some(session) => {
                    self.request_with_session_once(body, id, deadline, session.clone())
                        .await
                }
                None => self.request_once(body, id, deadline).await,
            };
            match attempt {
                Err(RequestAttemptError::RateLimited {
                    retry_after,
                    error: _,
                }) if retries < MAX_INITIALIZE_RATE_LIMIT_RETRIES
                    && retry_after < remaining(deadline) =>
                {
                    retries += 1;
                    if !retry_after.is_zero() {
                        sleep(retry_after).await;
                    }
                }
                other => return other,
            }
        }
    }

    async fn send_request_until(
        &self,
        body: JsonRpcRequest,
        id: u64,
        method: &str,
        deadline: Instant,
    ) -> Result<Value, McpError> {
        let first_attempt = if method == "initialize" {
            self.request_initialize_with_rate_limit_retry(&body, id, deadline, None)
                .await
        } else {
            self.request_once(&body, id, deadline).await
        };
        let parsed = match first_attempt {
            Ok(parsed) => parsed,
            Err(RequestAttemptError::SessionNotFound { session, .. }) if method != "initialize" => {
                self.recover_session(&session, deadline).await?;
                self.request_once(&body, id, deadline)
                    .await
                    .map_err(request_attempt_into_mcp)?
            }
            Err(error) => return Err(request_attempt_into_mcp(error)),
        };
        if let Some(error) = parsed.response.error {
            return Err(McpError::ServerError {
                code: error.code,
                message: error.message,
            });
        }
        let result = parsed
            .response
            .result
            .ok_or_else(|| McpError::Protocol("empty result".to_string()))?;
        if method == "initialize" {
            let version = result
                .get("protocolVersion")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    McpError::HandshakeFailed(
                        "initialize result omitted protocolVersion".to_string(),
                    )
                })?;
            self.validate_negotiated_protocol_version(version)?;
            self.install_initialized_state(parsed.initialize_session_id, version)
                .await;
            *self.initialize_id.lock().await = Some(id);
            *self.initialize_params.lock().await = body.params;
        }
        Ok(result)
    }

    async fn send_cancelled(&self, id: u64, deadline: Instant) {
        let _ = self
            .send_notification_until(
                "notifications/cancelled",
                Some(serde_json::json!({ "requestId": id })),
                deadline,
            )
            .await;
    }

    async fn send_notification_until(
        &self,
        method: &str,
        params: Option<Value>,
        deadline: Instant,
    ) -> Result<(), McpError> {
        let body = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let sent = self.post(&body, deadline).await?;
        self.validate_notification_response(sent, deadline).await
    }

    async fn send_notification_with_session_until(
        &self,
        method: &str,
        params: Option<Value>,
        deadline: Instant,
        session: SessionSnapshot,
    ) -> Result<(), McpError> {
        let body = JsonRpcNotification {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params,
        };
        let sent = self.post_with_session(&body, deadline, session).await?;
        self.validate_notification_response(sent, deadline).await
    }

    async fn validate_notification_response(
        &self,
        sent: SentResponse,
        deadline: Instant,
    ) -> Result<(), McpError> {
        validate_response_session_id(sent.response.headers())?;
        if sent.response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(McpError::Http(format!(
                "mcp notification returned {}",
                sent.response.status()
            )));
        }
        let body = timeout(remaining(deadline), sent.response.bytes())
            .await
            .map_err(|_| McpError::Timeout {
                timeout_ms: self.timeout_ms,
            })?
            .map_err(|e| McpError::Http(format_error(&e)))?;
        if !body.is_empty() {
            return Err(McpError::Protocol(
                "mcp notification response must have an empty body".to_string(),
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn send_request(
        &self,
        id: u64,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, McpError> {
        let _request = self.begin_request()?;
        let body = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.to_string(),
            params,
        };
        let deadline = overall_deadline(self.timeout_ms);
        let result = self.send_request_until(body, id, method, deadline).await;
        if matches!(result, Err(McpError::Timeout { .. })) && method != "initialize" {
            let cancellation_deadline =
                overall_deadline(self.timeout_ms.clamp(1, MAX_CANCELLATION_TIMEOUT_MS));
            self.send_cancelled(id, cancellation_deadline).await;
        }
        result
    }
    async fn send_notification(&self, method: &str, params: Option<Value>) -> Result<(), McpError> {
        let _request = self.begin_request()?;
        self.send_notification_until(method, params, overall_deadline(self.timeout_ms))
            .await
    }

    fn initialize_protocol_version(&self) -> &'static str {
        "2025-11-25"
    }

    fn validate_negotiated_protocol_version(&self, version: &str) -> Result<(), McpError> {
        if version == "2025-11-25" {
            Ok(())
        } else {
            Err(McpError::HandshakeFailed(format!(
                "unsupported Streamable HTTP MCP protocol version `{version}`"
            )))
        }
    }

    async fn set_protocol_version(&self, protocol_version: &str) {
        if let Ok(value) = HeaderValue::from_str(protocol_version) {
            self.session
                .lock()
                .expect("session mutex poisoned")
                .protocol_version = Some(value);
        }
    }

    async fn close(&self) -> Result<(), McpError> {
        self.close_session().await
    }
}

fn request_attempt_into_mcp(error: RequestAttemptError) -> McpError {
    match error {
        RequestAttemptError::Mcp(error)
        | RequestAttemptError::RateLimited { error, .. }
        | RequestAttemptError::SessionNotFound { error, .. } => error,
    }
}

async fn send_timed(
    request: RequestBuilder,
    deadline: Instant,
    timeout_ms: u64,
) -> Result<Response, McpError> {
    match timeout(remaining(deadline), request.send()).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(McpError::Http(format_error(&error))),
        Err(_) => Err(McpError::Timeout { timeout_ms }),
    }
}

fn overall_deadline(timeout_ms: u64) -> Instant {
    Instant::now() + std::time::Duration::from_millis(timeout_ms)
}

fn remaining(deadline: Instant) -> std::time::Duration {
    deadline
        .checked_duration_since(Instant::now())
        .unwrap_or(std::time::Duration::ZERO)
}

fn retry_after_delay(headers: &HeaderMap) -> std::time::Duration {
    headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(std::time::Duration::from_secs)
        .unwrap_or(DEFAULT_INITIALIZE_RATE_LIMIT_RETRY_DELAY)
}

fn validate_response_session_id(headers: &HeaderMap) -> Result<Option<HeaderValue>, McpError> {
    let Some(session_id) = headers.get(MCP_SESSION_ID) else {
        return Ok(None);
    };
    let bytes = session_id.as_bytes();
    if bytes.is_empty() || !bytes.iter().all(|byte| (0x21..=0x7e).contains(byte)) {
        return Err(McpError::Protocol(
            "invalid MCP-Session-Id response header: expected non-empty visible ASCII bytes \
             (0x21..=0x7E)"
                .to_string(),
        ));
    }
    Ok(Some(session_id.clone()))
}

fn validate_sse_content_type(headers: &HeaderMap) -> Result<(), McpError> {
    let is_event_stream = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("text/event-stream"));
    if is_event_stream {
        Ok(())
    } else {
        Err(McpError::Protocol(
            "SSE recovery response Content-Type must be text/event-stream".to_string(),
        ))
    }
}

struct SseEvent {
    id: Option<SseEventId>,
    retry: Option<std::time::Duration>,
    response: Option<JsonRpcResponse>,
}

enum SseEventId {
    Set(String),
    Reset,
}

fn parse_sse_event(raw_event: &str) -> Result<SseEvent, McpError> {
    let mut id = None;
    let mut retry = None;
    let mut data = String::new();
    for line in raw_event.lines() {
        if let Some(value) = line.strip_prefix("id:") {
            let value = value.trim();
            id = Some(if value.is_empty() {
                SseEventId::Reset
            } else {
                SseEventId::Set(value.to_string())
            });
        } else if let Some(value) = line.strip_prefix("retry:") {
            retry = value
                .trim()
                .parse::<u64>()
                .ok()
                .map(std::time::Duration::from_millis);
        } else if let Some(value) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.trim_start());
        }
    }
    if data.trim().is_empty() {
        return Ok(SseEvent {
            id,
            retry,
            response: None,
        });
    }
    let message: Value = serde_json::from_str(&data)
        .map_err(|e| McpError::Protocol(format!("parse SSE JSON-RPC message: {e}")))?;
    let has_method = message.get("method").is_some();
    let has_response = message.get("result").is_some() || message.get("error").is_some();
    let response = match (has_method, has_response) {
        (true, false) => None,
        (false, true) => Some(
            serde_json::from_value(message)
                .map_err(|e| McpError::Protocol(format!("parse SSE JSON-RPC response: {e}")))?,
        ),
        _ => {
            return Err(McpError::Protocol(
                "SSE JSON-RPC message must contain either method or result/error".to_string(),
            ));
        }
    };
    Ok(SseEvent {
        id,
        retry,
        response,
    })
}

fn normalize_crlf(buffer: &mut Vec<u8>) {
    if buffer.is_empty() {
        return;
    }
    let end = if buffer.last() == Some(&b'\r') {
        buffer.len() - 1
    } else {
        buffer.len()
    };
    let mut read = 0;
    let mut write = 0;
    while read < end {
        if buffer[read] == b'\r' {
            buffer[write] = b'\n';
            write += 1;
            read += 1;
            if read < end && buffer[read] == b'\n' {
                read += 1;
            }
        } else {
            if read != write {
                buffer[write] = buffer[read];
            }
            write += 1;
            read += 1;
        }
    }
    while read < buffer.len() {
        if read != write {
            buffer[write] = buffer[read];
        }
        write += 1;
        read += 1;
    }
    buffer.truncate(write);
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn format_error(error: &reqwest::Error) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(error) = source {
        message.push_str(": ");
        message.push_str(&error.to_string());
        source = error.source();
    }
    message
}

#[cfg(test)]
mod tests {
    use super::{
        overall_deadline, remaining, retry_after_delay, DEFAULT_INITIALIZE_RATE_LIMIT_RETRY_DELAY,
    };
    use reqwest::header::{HeaderMap, HeaderValue};
    use std::time::Duration;
    use tokio::time::Instant;

    #[test]
    fn non_initialize_request_uses_the_full_configured_deadline() {
        let timeout = Duration::from_millis(1_000);
        let earliest = Instant::now() + timeout;
        let request_deadline = overall_deadline(1_000);
        let latest = Instant::now() + timeout;

        assert!(request_deadline >= earliest);
        assert!(request_deadline <= latest);
    }

    #[test]
    fn rate_limit_retry_delay_uses_retry_after_seconds_or_a_small_fallback() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", HeaderValue::from_static("2"));
        assert_eq!(retry_after_delay(&headers), Duration::from_secs(2));

        headers.insert("retry-after", HeaderValue::from_static("invalid"));
        assert_eq!(
            retry_after_delay(&headers),
            DEFAULT_INITIALIZE_RATE_LIMIT_RETRY_DELAY
        );
    }

    #[test]
    fn elapsed_deadline_has_no_remaining_grace_period() {
        let deadline = Instant::now() - Duration::from_millis(1);

        assert_eq!(remaining(deadline), Duration::ZERO);
    }
}
