use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::extract::{Json, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::Router;
use futures_util::stream;
use mcp::{EffectSection, McpClient, McpError, McpServerConfig, Transport};
use serde_json::{json, Value};
use tokio::sync::{Barrier, Mutex, Notify};

static ENV_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    headers: HeaderMap,
    body: Value,
}

struct FixtureServer {
    url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    recovery_started: Arc<Notify>,
    release_recovery: Arc<Notify>,
    first_stale_request_seen: Arc<Notify>,
    release_first_stale_request: Arc<Notify>,
}

struct RedirectFixture {
    url: String,
    redirected_requests: Arc<Mutex<Vec<HeaderMap>>>,
}

#[derive(Clone)]
struct RedirectOriginState {
    target_url: String,
}

#[derive(Clone)]
struct FixtureState {
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    list_response: ListResponse,
    init_protocol_version: String,
    list_calls: Arc<AtomicUsize>,
    initialize_calls: Arc<AtomicUsize>,
    list_request_id: Arc<Mutex<Option<Value>>>,
    resume_barrier: Arc<Barrier>,
    session_recovery_barrier: Arc<Barrier>,
    recovered_request_seen: Arc<Notify>,
    recovery_started: Arc<Notify>,
    release_recovery: Arc<Notify>,
    first_stale_request_seen: Arc<Notify>,
    release_first_stale_request: Arc<Notify>,
}

#[derive(Clone, Copy)]
enum ListResponse {
    Correct,
    WrongId,
    UnmatchedThenCorrect,
    SessionExpiredThenCorrect,
    SessionAlwaysExpired,
    TimeoutHeadersAndBody,
    NeverCompletes,
    InitializeNeverCompletes,
    BadNotification,
    WrongNotificationStatus,
    UppercaseSse,
    ResumableSse,
    RetryExceedsDeadline,
    DisconnectWithoutEventId,
    ResumeStillDisconnects,
    WrongRecoveryContentType,
    ConcurrentResumableSse,
    GetSessionExpiredThenCorrect,
    GetSessionRecoveryExceedsDeadline,
    ConcurrentSessionExpiry,
    RecoveryBlocksConcurrentRequest,
    ConcurrentRecoveryFailure,
    RecoveryMutexDeadline,
    FailedInitializeSession,
    EmptySessionId,
    InvalidSessionId,
    ServerRequestThenResponse,
    EmptyEventIdResets,
}

impl FixtureServer {
    async fn json_then_sse() -> Self {
        Self::start(ListResponse::Correct, "2025-11-25").await
    }

    async fn wrong_list_id() -> Self {
        Self::start(ListResponse::WrongId, "2025-11-25").await
    }

    async fn unmatched_then_correct_list_id() -> Self {
        Self::start(ListResponse::UnmatchedThenCorrect, "2025-11-25").await
    }

    async fn unsupported_protocol() -> Self {
        Self::start(ListResponse::Correct, "2024-11-05").await
    }

    async fn session_expiry() -> Self {
        Self::start(ListResponse::SessionExpiredThenCorrect, "2025-11-25").await
    }

    async fn persistent_session_expiry() -> Self {
        Self::start(ListResponse::SessionAlwaysExpired, "2025-11-25").await
    }

    async fn timeout_headers_and_body() -> Self {
        Self::start(ListResponse::TimeoutHeadersAndBody, "2025-11-25").await
    }

    async fn never_completes() -> Self {
        Self::start(ListResponse::NeverCompletes, "2025-11-25").await
    }

    async fn initialize_never_completes() -> Self {
        Self::start(ListResponse::InitializeNeverCompletes, "2025-11-25").await
    }

    async fn bad_notification() -> Self {
        Self::start(ListResponse::BadNotification, "2025-11-25").await
    }

    async fn wrong_notification_status() -> Self {
        Self::start(ListResponse::WrongNotificationStatus, "2025-11-25").await
    }

    async fn uppercase_sse() -> Self {
        Self::start(ListResponse::UppercaseSse, "2025-11-25").await
    }

    async fn resumable_sse() -> Self {
        Self::start(ListResponse::ResumableSse, "2025-11-25").await
    }

    async fn sse_retry_exceeds_deadline() -> Self {
        Self::start(ListResponse::RetryExceedsDeadline, "2025-11-25").await
    }

    async fn disconnect_without_event_id() -> Self {
        Self::start(ListResponse::DisconnectWithoutEventId, "2025-11-25").await
    }

    async fn resume_still_disconnects() -> Self {
        Self::start(ListResponse::ResumeStillDisconnects, "2025-11-25").await
    }

    async fn wrong_recovery_content_type() -> Self {
        Self::start(ListResponse::WrongRecoveryContentType, "2025-11-25").await
    }

    async fn concurrent_resumable_sse() -> Self {
        Self::start(ListResponse::ConcurrentResumableSse, "2025-11-25").await
    }

    async fn get_session_expiry() -> Self {
        Self::start(ListResponse::GetSessionExpiredThenCorrect, "2025-11-25").await
    }

    async fn slow_get_session_expiry() -> Self {
        Self::start(
            ListResponse::GetSessionRecoveryExceedsDeadline,
            "2025-11-25",
        )
        .await
    }

    async fn concurrent_session_expiry() -> Self {
        Self::start(ListResponse::ConcurrentSessionExpiry, "2025-11-25").await
    }

    async fn recovery_blocks_concurrent_request() -> Self {
        Self::start(ListResponse::RecoveryBlocksConcurrentRequest, "2025-11-25").await
    }

    async fn concurrent_recovery_failure() -> Self {
        Self::start(ListResponse::ConcurrentRecoveryFailure, "2025-11-25").await
    }

    async fn recovery_mutex_deadline() -> Self {
        Self::start(ListResponse::RecoveryMutexDeadline, "2025-11-25").await
    }

    async fn failed_initialize_session() -> Self {
        Self::start(ListResponse::FailedInitializeSession, "2025-11-25").await
    }

    async fn empty_session_id() -> Self {
        Self::start(ListResponse::EmptySessionId, "2025-11-25").await
    }

    async fn invalid_session_id() -> Self {
        Self::start(ListResponse::InvalidSessionId, "2025-11-25").await
    }

    async fn server_request_then_response() -> Self {
        Self::start(ListResponse::ServerRequestThenResponse, "2025-11-25").await
    }

    async fn empty_event_id_resets() -> Self {
        Self::start(ListResponse::EmptyEventIdResets, "2025-11-25").await
    }

    async fn start(list_response: ListResponse, init_protocol_version: &str) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recovery_started = Arc::new(Notify::new());
        let release_recovery = Arc::new(Notify::new());
        let first_stale_request_seen = Arc::new(Notify::new());
        let release_first_stale_request = Arc::new(Notify::new());
        let state = FixtureState {
            requests: Arc::clone(&requests),
            list_response,
            init_protocol_version: init_protocol_version.to_string(),
            list_calls: Arc::new(AtomicUsize::new(0)),
            initialize_calls: Arc::new(AtomicUsize::new(0)),
            list_request_id: Arc::new(Mutex::new(None)),
            resume_barrier: Arc::new(Barrier::new(2)),
            session_recovery_barrier: Arc::new(Barrier::new(2)),
            recovered_request_seen: Arc::new(Notify::new()),
            recovery_started: Arc::clone(&recovery_started),
            release_recovery: Arc::clone(&release_recovery),
            first_stale_request_seen: Arc::clone(&first_stale_request_seen),
            release_first_stale_request: Arc::clone(&release_first_stale_request),
        };
        let app = Router::new()
            .route(
                "/mcp",
                post(handle_request).get(handle_get).delete(handle_delete),
            )
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            url: format!("http://{address}/mcp"),
            requests,
            recovery_started,
            release_recovery,
            first_stale_request_seen,
            release_first_stale_request,
        }
    }

    fn config(&self) -> McpServerConfig {
        McpServerConfig {
            name: "ram-a".to_string(),
            transport: Transport::StreamableHttp,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(self.url.clone()),
            bearer_token_env: Some("MCP_STREAMABLE_HTTP_TEST_TOKEN".to_string()),
            agent_id: Some("xiaoo-test-agent".to_string()),
            headers: BTreeMap::from([(
                "X-XiaoO-Client".to_string(),
                "integration-test".to_string(),
            )]),
            enabled: None,
            timeout_ms: 1_000,
            effect: EffectSection::default(),
        }
    }

    async fn last_header(&self, method: &str, name: &str) -> Option<String> {
        self.requests
            .lock()
            .await
            .iter()
            .rev()
            .find(|request| request.method == method)
            .and_then(|request| request.headers.get(name))
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
    }

    async fn last_body(&self, method: &str) -> Option<Value> {
        self.requests
            .lock()
            .await
            .iter()
            .rev()
            .find(|request| request.method == method)
            .map(|request| request.body.clone())
    }

    async fn count_method(&self, method: &str) -> usize {
        self.requests
            .lock()
            .await
            .iter()
            .filter(|request| request.method == method)
            .count()
    }
}

impl RedirectFixture {
    async fn start() -> Self {
        let redirected_requests = Arc::new(Mutex::new(Vec::new()));
        let sink_requests = Arc::clone(&redirected_requests);
        let sink = Router::new()
            .route(
                "/capture",
                post(
                    |State(requests): State<Arc<Mutex<Vec<HeaderMap>>>>,
                     headers: HeaderMap,
                     Json(request): Json<Value>| async move {
                        requests.lock().await.push(headers);
                        Json(json!({
                            "jsonrpc": "2.0",
                            "id": request["id"],
                            "result": {"tools": []}
                        }))
                    },
                ),
            )
            .with_state(sink_requests);
        let sink_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let sink_address = sink_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(sink_listener, sink).await.unwrap();
        });

        let origin = Router::new()
            .route("/mcp", post(handle_redirect_origin))
            .with_state(RedirectOriginState {
                target_url: format!("http://{sink_address}/capture"),
            });
        let origin_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin_address = origin_listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(origin_listener, origin).await.unwrap();
        });

        Self {
            url: format!("http://{origin_address}/mcp"),
            redirected_requests,
        }
    }

    fn config(&self) -> McpServerConfig {
        McpServerConfig {
            name: "redirect-test".to_string(),
            transport: Transport::StreamableHttp,
            command: None,
            args: Vec::new(),
            env: HashMap::new(),
            url: Some(self.url.clone()),
            bearer_token_env: Some("MCP_STREAMABLE_HTTP_TEST_TOKEN".to_string()),
            agent_id: Some("xiaoo-test-agent".to_string()),
            headers: BTreeMap::from([(
                "X-XiaoO-Client".to_string(),
                "integration-test".to_string(),
            )]),
            enabled: None,
            timeout_ms: 1_000,
            effect: EffectSection::default(),
        }
    }
}

async fn handle_redirect_origin(
    State(state): State<RedirectOriginState>,
    Json(request): Json<Value>,
) -> Response {
    match request["method"].as_str().unwrap() {
        "initialize" => (
            StatusCode::OK,
            [(
                "mcp-session-id",
                HeaderValue::from_static("redirect-session"),
            )],
            Json(json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "serverInfo": {"name": "fixture", "version": "1"}
                }
            })),
        )
            .into_response(),
        "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", state.target_url)],
        )
            .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn handle_request(
    State(state): State<FixtureState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    let method = request["method"].as_str().unwrap().to_string();
    state.requests.lock().await.push(CapturedRequest {
        method: method.clone(),
        headers: headers.clone(),
        body: request.clone(),
    });

    match method.as_str() {
        "initialize" => {
            if matches!(state.list_response, ListResponse::InitializeNeverCompletes) {
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            let initialize_call = state.initialize_calls.fetch_add(1, Ordering::SeqCst);
            if matches!(
                state.list_response,
                ListResponse::GetSessionRecoveryExceedsDeadline
            ) && initialize_call > 0
            {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            if initialize_call > 0 {
                match state.list_response {
                    ListResponse::RecoveryBlocksConcurrentRequest => {
                        state.recovery_started.notify_one();
                        state.release_recovery.notified().await;
                    }
                    ListResponse::ConcurrentRecoveryFailure => {
                        state.recovery_started.notify_one();
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        return (
                            StatusCode::OK,
                            Json(json!({
                                "jsonrpc": "2.0",
                                "id": request["id"],
                                "error": {"code": -32001, "message": "recovery failed"}
                            })),
                        )
                            .into_response();
                    }
                    ListResponse::RecoveryMutexDeadline => {
                        state.recovery_started.notify_one();
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    _ => {}
                }
            }
            if matches!(state.list_response, ListResponse::FailedInitializeSession)
                && initialize_call == 0
            {
                return (
                    StatusCode::OK,
                    [("mcp-session-id", HeaderValue::from_static("failed-session"))],
                    Json(json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "error": {"code": -32000, "message": "initialize failed"}
                    })),
                )
                    .into_response();
            }
            let response = Json(json!({
                "jsonrpc": "2.0",
                "id": request["id"],
                "result": {
                    "protocolVersion": state.init_protocol_version,
                    "capabilities": {},
                    "serverInfo": {"name": "fixture", "version": "1"}
                }
            }));
            if matches!(state.list_response, ListResponse::FailedInitializeSession) {
                return response.into_response();
            }
            let invalid_session_id = match state.list_response {
                ListResponse::EmptySessionId => Some(HeaderValue::from_static("")),
                ListResponse::InvalidSessionId => Some(HeaderValue::from_static("bad id")),
                _ => None,
            };
            if let Some(session_id) = invalid_session_id {
                return (StatusCode::OK, [("mcp-session-id", session_id)], response)
                    .into_response();
            }
            let session_id = if matches!(
                state.list_response,
                ListResponse::GetSessionExpiredThenCorrect
                    | ListResponse::GetSessionRecoveryExceedsDeadline
                    | ListResponse::ConcurrentSessionExpiry
                    | ListResponse::RecoveryBlocksConcurrentRequest
                    | ListResponse::ConcurrentRecoveryFailure
                    | ListResponse::RecoveryMutexDeadline
            ) {
                format!("fixture-session-{}", initialize_call + 1)
            } else {
                "fixture-session".to_string()
            };
            (
                StatusCode::OK,
                [(
                    "mcp-session-id",
                    HeaderValue::from_str(&session_id).unwrap(),
                )],
                response,
            )
                .into_response()
        }
        "notifications/initialized" => {
            if matches!(state.list_response, ListResponse::BadNotification) {
                return (StatusCode::ACCEPTED, "unexpected-body").into_response();
            }
            if matches!(state.list_response, ListResponse::WrongNotificationStatus) {
                return StatusCode::OK.into_response();
            }
            StatusCode::ACCEPTED.into_response()
        }
        "notifications/cancelled" => StatusCode::ACCEPTED.into_response(),
        "tools/list" => {
            let session_id = headers
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
                .map(ToString::to_string);
            if matches!(
                state.list_response,
                ListResponse::GetSessionExpiredThenCorrect
                    | ListResponse::GetSessionRecoveryExceedsDeadline
            ) && session_id.as_deref() == Some("fixture-session-1")
            {
                *state.list_request_id.lock().await = Some(request["id"].clone());
                let event = Bytes::from(
                    "id: stale-event\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n",
                );
                let body = if matches!(
                    state.list_response,
                    ListResponse::GetSessionRecoveryExceedsDeadline
                ) {
                    Body::from_stream(stream::once(async move {
                        tokio::time::sleep(Duration::from_millis(30)).await;
                        Ok::<Bytes, std::convert::Infallible>(event)
                    }))
                } else {
                    Body::from(event)
                };
                return (
                    StatusCode::OK,
                    [("content-type", "text/event-stream")],
                    body,
                )
                    .into_response();
            }
            if matches!(state.list_response, ListResponse::ConcurrentSessionExpiry) {
                if session_id.as_deref() == Some("fixture-session-1") {
                    let old_request = state.list_calls.fetch_add(1, Ordering::SeqCst);
                    state.session_recovery_barrier.wait().await;
                    if old_request == 0 {
                        return StatusCode::NOT_FOUND.into_response();
                    }
                    state.recovered_request_seen.notified().await;
                    return StatusCode::NOT_FOUND.into_response();
                }
                if session_id.as_deref() == Some("fixture-session-2") {
                    state.recovered_request_seen.notify_waiters();
                }
            }
            if matches!(
                state.list_response,
                ListResponse::RecoveryBlocksConcurrentRequest
            ) && session_id.as_deref() == Some("fixture-session-1")
            {
                return StatusCode::NOT_FOUND.into_response();
            }
            if matches!(state.list_response, ListResponse::ConcurrentRecoveryFailure)
                && session_id.as_deref() == Some("fixture-session-1")
            {
                state.session_recovery_barrier.wait().await;
                return StatusCode::NOT_FOUND.into_response();
            }
            if matches!(state.list_response, ListResponse::RecoveryMutexDeadline)
                && session_id.as_deref() == Some("fixture-session-1")
            {
                if state.list_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    state.first_stale_request_seen.notify_one();
                    state.release_first_stale_request.notified().await;
                }
                return StatusCode::NOT_FOUND.into_response();
            }
            if matches!(state.list_response, ListResponse::SessionExpiredThenCorrect)
                && state.list_calls.fetch_add(1, Ordering::Relaxed) == 0
            {
                return StatusCode::NOT_FOUND.into_response();
            }
            if matches!(state.list_response, ListResponse::SessionAlwaysExpired) {
                return StatusCode::NOT_FOUND.into_response();
            }
            let response = |id| json!({"jsonrpc": "2.0", "id": id, "result": {"tools": []}});
            if matches!(state.list_response, ListResponse::TimeoutHeadersAndBody) {
                tokio::time::sleep(Duration::from_millis(40)).await;
                let event = format!(
                    "event: message\ndata: {}\n\n",
                    response(request["id"].clone())
                );
                let body = Body::from_stream(stream::once(async move {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok::<Bytes, std::convert::Infallible>(Bytes::from(event))
                }));
                return (
                    StatusCode::OK,
                    [("content-type", "text/event-stream")],
                    body,
                )
                    .into_response();
            }
            if matches!(state.list_response, ListResponse::NeverCompletes) {
                let body = Body::from_stream(stream::once(async {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    Ok::<Bytes, std::convert::Infallible>(Bytes::new())
                }));
                return (
                    StatusCode::OK,
                    [("content-type", "text/event-stream")],
                    body,
                )
                    .into_response();
            }
            if matches!(state.list_response, ListResponse::ConcurrentResumableSse) {
                let event = Bytes::from(format!(
                    "id: event-{}\nevent: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{}}}}\n\n",
                    request["id"]
                ));
                let barrier = Arc::clone(&state.resume_barrier);
                let body = Body::from_stream(stream::unfold(
                    (0_u8, event, barrier),
                    |(step, event, barrier)| async move {
                        match step {
                            0 => Some((
                                Ok::<Bytes, std::convert::Infallible>(event.clone()),
                                (1, event, barrier),
                            )),
                            1 => {
                                barrier.wait().await;
                                tokio::time::sleep(Duration::from_millis(100)).await;
                                Some((
                                    Ok::<Bytes, std::convert::Infallible>(Bytes::new()),
                                    (2, event, barrier),
                                ))
                            }
                            _ => None,
                        }
                    },
                ));
                return (
                    StatusCode::OK,
                    [("content-type", "text/event-stream")],
                    body,
                )
                    .into_response();
            }
            let events = match state.list_response {
                ListResponse::Correct => format!(
                    "event: message\ndata: {}\n\n",
                    response(request["id"].clone())
                ),
                ListResponse::SessionExpiredThenCorrect => format!(
                    "event: message\ndata: {}\n\n",
                    response(request["id"].clone())
                ),
                ListResponse::SessionAlwaysExpired => unreachable!(),
                ListResponse::TimeoutHeadersAndBody | ListResponse::NeverCompletes => {
                    unreachable!()
                }
                ListResponse::InitializeNeverCompletes
                | ListResponse::BadNotification
                | ListResponse::WrongNotificationStatus => {
                    unreachable!()
                }
                ListResponse::UppercaseSse => format!(
                    "event: message\ndata: {}\n\n",
                    response(request["id"].clone())
                ),
                ListResponse::ResumableSse => {
                    *state.list_request_id.lock().await = Some(request["id"].clone());
                    "id: event-1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
                }
                ListResponse::RetryExceedsDeadline => {
                    *state.list_request_id.lock().await = Some(request["id"].clone());
                    "id: event-1\nretry: 1000\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
                }
                ListResponse::DisconnectWithoutEventId => {
                    "event: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
                }
                ListResponse::ResumeStillDisconnects => {
                    *state.list_request_id.lock().await = Some(request["id"].clone());
                    "id: event-1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
                }
                ListResponse::WrongRecoveryContentType => {
                    *state.list_request_id.lock().await = Some(request["id"].clone());
                    "id: event-1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
                }
                ListResponse::ConcurrentResumableSse => unreachable!(),
                ListResponse::GetSessionExpiredThenCorrect
                | ListResponse::GetSessionRecoveryExceedsDeadline
                | ListResponse::ConcurrentSessionExpiry
                | ListResponse::RecoveryBlocksConcurrentRequest
                | ListResponse::ConcurrentRecoveryFailure
                | ListResponse::RecoveryMutexDeadline
                | ListResponse::FailedInitializeSession
                | ListResponse::EmptySessionId
                | ListResponse::InvalidSessionId => format!(
                    "event: message\ndata: {}\n\n",
                    response(request["id"].clone())
                ),
                ListResponse::ServerRequestThenResponse => format!(
                    "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                    json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "method": "roots/list",
                        "params": {}
                    }),
                    response(request["id"].clone()),
                ),
                ListResponse::EmptyEventIdResets => {
                    *state.list_request_id.lock().await = Some(request["id"].clone());
                    "id: event-1\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\nid:\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
                }
                ListResponse::WrongId => {
                    format!("event: message\ndata: {}\n\n", response(json!(999)))
                }
                ListResponse::UnmatchedThenCorrect => format!(
                    "event: message\ndata: {}\n\nevent: message\ndata: {}\n\n",
                    response(json!(999)),
                    response(request["id"].clone()),
                ),
            };
            let content_type = if matches!(state.list_response, ListResponse::UppercaseSse) {
                "Text/Event-Stream; charset=utf-8"
            } else {
                "text/event-stream"
            };
            (StatusCode::OK, [("content-type", content_type)], events).into_response()
        }
        "tools/call" => Json(json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "result": {
                "content": [],
                "structuredContent": {"answer": 42}
            }
        }))
        .into_response(),
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn handle_get(State(state): State<FixtureState>, headers: HeaderMap) -> Response {
    state.requests.lock().await.push(CapturedRequest {
        method: "GET".to_string(),
        headers: headers.clone(),
        body: json!({}),
    });
    if matches!(
        state.list_response,
        ListResponse::GetSessionExpiredThenCorrect
            | ListResponse::GetSessionRecoveryExceedsDeadline
    ) {
        if matches!(
            state.list_response,
            ListResponse::GetSessionRecoveryExceedsDeadline
        ) {
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
        return StatusCode::NOT_FOUND.into_response();
    }
    let id = if matches!(state.list_response, ListResponse::ConcurrentResumableSse) {
        headers
            .get("last-event-id")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("event-"))
            .and_then(|value| value.parse::<u64>().ok())
            .map(Value::from)
            .unwrap_or_else(|| json!(999))
    } else {
        state
            .list_request_id
            .lock()
            .await
            .clone()
            .unwrap_or_else(|| json!(2))
    };
    let body = if matches!(state.list_response, ListResponse::ResumeStillDisconnects) {
        "id: event-2\nevent: message\ndata: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n".to_string()
    } else {
        format!(
            "id: event-2\nevent: message\ndata: {}\n\n",
            json!({"jsonrpc": "2.0", "id": id, "result": {"tools": []}})
        )
    };
    let content_type = if matches!(state.list_response, ListResponse::WrongRecoveryContentType) {
        "application/json"
    } else {
        "text/event-stream"
    };
    (StatusCode::OK, [("content-type", content_type)], body).into_response()
}

async fn handle_delete(State(state): State<FixtureState>, headers: HeaderMap) -> Response {
    state.requests.lock().await.push(CapturedRequest {
        method: "DELETE".to_string(),
        headers,
        body: json!({}),
    });
    StatusCode::ACCEPTED.into_response()
}

#[tokio::test]
async fn initialize_captures_session_and_subsequent_calls_send_streamable_headers() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "old-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::json_then_sse().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");

    let initialized = client.initialize().await.unwrap();
    assert_eq!(initialized.protocol_version, "2025-11-25");
    assert_eq!(
        server
            .last_body("initialize")
            .await
            .expect("initialize request")
            .pointer("/params/protocolVersion")
            .and_then(Value::as_str),
        Some("2025-11-25")
    );
    client.list_tools().await.unwrap();

    assert_eq!(
        server
            .last_header("tools/list", "mcp-session-id")
            .await
            .as_deref(),
        Some("fixture-session")
    );
    assert_eq!(
        server
            .last_header("tools/list", "mcp-protocol-version")
            .await
            .as_deref(),
        Some("2025-11-25")
    );
    assert_eq!(
        server
            .last_header("tools/list", "authorization")
            .await
            .as_deref(),
        Some("Bearer test-token")
    );
    assert_eq!(
        server
            .last_header("tools/list", "x-agent-id")
            .await
            .as_deref(),
        Some("xiaoo-test-agent")
    );
    assert_eq!(
        server.last_header("tools/list", "accept").await.as_deref(),
        Some("application/json, text/event-stream")
    );
    assert_eq!(server.last_header("tools/list", "mcp-method").await, None);
    assert_eq!(server.last_header("tools/list", "mcp-name").await, None);

    let tool_result = client.call_tool("structured", json!({})).await.unwrap();
    assert_eq!(tool_result.structured_content, Some(json!({"answer": 42})));
    assert_eq!(tool_result.flatten_text(), r#"{"answer":42}"#);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn close_deletes_the_active_streamable_http_session_once() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::json_then_sse().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    client.close().await.unwrap();
    client.close().await.unwrap();

    assert_eq!(server.count_method("DELETE").await, 1);
    assert_eq!(
        server
            .last_header("DELETE", "mcp-session-id")
            .await
            .as_deref(),
        Some("fixture-session")
    );
    assert_eq!(
        server
            .last_header("DELETE", "mcp-protocol-version")
            .await
            .as_deref(),
        Some("2025-11-25")
    );
    assert_eq!(
        server
            .last_header("DELETE", "authorization")
            .await
            .as_deref(),
        Some("Bearer test-token")
    );
    assert_eq!(
        server.last_header("DELETE", "x-agent-id").await.as_deref(),
        Some("xiaoo-test-agent")
    );
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn close_waits_for_session_recovery_then_deletes_the_recovered_session() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::recovery_blocks_concurrent_request().await;
    let client = Arc::new(McpClient::connect(&server.config()).await.unwrap());
    client.initialize().await.unwrap();

    let recovering_request = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.list_tools().await }
    });
    server.recovery_started.notified().await;
    let close = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.close().await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(server.count_method("DELETE").await, 0);

    server.release_recovery.notify_one();
    assert!(recovering_request.await.unwrap().is_ok());
    assert!(close.await.unwrap().is_ok());
    assert_eq!(server.count_method("DELETE").await, 1);
    assert_eq!(
        server
            .last_header("DELETE", "mcp-session-id")
            .await
            .as_deref(),
        Some("fixture-session-2")
    );
    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Disconnected)
    ));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_an_unsupported_streamable_http_protocol_version() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::unsupported_protocol().await;
    let client = McpClient::connect(&server.config()).await.unwrap();

    let error = client.initialize().await.unwrap_err();
    assert!(matches!(error, McpError::HandshakeFailed(message) if message.contains("2024-11-05")));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn reinitializes_once_when_a_session_request_returns_not_found() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::session_expiry().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(client.list_tools().await.is_ok());
    assert_eq!(server.count_method("initialize").await, 2);
    assert_eq!(server.count_method("tools/list").await, 2);
    assert_eq!(
        server
            .last_header("tools/list", "mcp-session-id")
            .await
            .as_deref(),
        Some("fixture-session")
    );
    assert_eq!(
        server.last_header("initialize", "mcp-session-id").await,
        None
    );
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn session_not_found_recovery_is_bounded_to_one_retry() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::persistent_session_expiry().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(matches!(client.list_tools().await, Err(McpError::Http(_))));
    assert_eq!(server.count_method("initialize").await, 2);
    assert_eq!(server.count_method("tools/list").await, 2);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn get_not_found_recovers_the_session_and_retries_the_original_request_once() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::get_session_expiry().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(client.list_tools().await.is_ok());
    assert_eq!(server.count_method("GET").await, 1);
    assert_eq!(server.count_method("initialize").await, 2);
    assert_eq!(server.count_method("tools/list").await, 2);
    assert_eq!(
        server
            .last_header("tools/list", "mcp-session-id")
            .await
            .as_deref(),
        Some("fixture-session-2")
    );
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn get_not_found_session_recovery_keeps_the_original_request_deadline() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::slow_get_session_expiry().await;
    let mut config = server.config();
    config.timeout_ms = 80;
    let client = McpClient::connect(&config).await.unwrap();
    client.initialize().await.unwrap();

    let started = tokio::time::Instant::now();
    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Timeout { .. })
    ));
    assert!(started.elapsed() < Duration::from_millis(120));
    assert_eq!(server.count_method("tools/list").await, 1);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn stale_concurrent_request_does_not_clear_a_newer_session() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::concurrent_session_expiry().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    let (first, second) = tokio::join!(client.list_tools(), client.list_tools());
    assert!(first.is_ok(), "first request failed: {first:?}");
    assert!(second.is_ok(), "second request failed: {second:?}");
    assert_eq!(server.count_method("initialize").await, 2);
    assert_eq!(server.count_method("tools/list").await, 4);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn requests_started_during_recovery_wait_and_use_the_recovered_session() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::recovery_blocks_concurrent_request().await;
    let client = Arc::new(McpClient::connect(&server.config()).await.unwrap());
    client.initialize().await.unwrap();

    let first = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.list_tools().await }
    });
    server.recovery_started.notified().await;
    let second = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.list_tools().await }
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(Duration::from_millis(20)).await;

    assert_eq!(server.count_method("tools/list").await, 1);
    assert!(!second.is_finished());
    server.release_recovery.notify_one();
    assert!(first.await.unwrap().is_ok());
    assert!(second.await.unwrap().is_ok());

    let requests = server.requests.lock().await;
    let session_ids = requests
        .iter()
        .filter(|request| request.method == "tools/list")
        .map(|request| {
            request
                .headers
                .get("mcp-session-id")
                .and_then(|value| value.to_str().ok())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        session_ids,
        vec![
            Some("fixture-session-1"),
            Some("fixture-session-2"),
            Some("fixture-session-2")
        ]
    );
    assert_eq!(
        requests
            .iter()
            .rfind(|request| request.method == "initialize")
            .and_then(|request| request.headers.get("mcp-session-id")),
        None
    );
    assert_eq!(
        requests
            .iter()
            .rfind(|request| request.method == "notifications/initialized")
            .and_then(|request| request.headers.get("mcp-session-id"))
            .and_then(|value| value.to_str().ok()),
        Some("fixture-session-2")
    );
    drop(requests);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn recovery_failure_is_returned_to_waiters_without_a_sessionless_retry() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::concurrent_recovery_failure().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    let (first, second) = tokio::join!(client.list_tools(), client.list_tools());
    assert!(
        matches!(first, Err(McpError::ServerError { code: -32001, .. })),
        "first request unexpectedly returned {first:?}"
    );
    assert!(
        matches!(second, Err(McpError::ServerError { code: -32001, .. })),
        "second request unexpectedly returned {second:?}"
    );
    assert_eq!(server.count_method("tools/list").await, 2);
    assert!(server
        .requests
        .lock()
        .await
        .iter()
        .filter(|request| request.method == "tools/list")
        .all(|request| request.headers.get("mcp-session-id").is_some()));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn waiting_for_the_recovery_mutex_is_bounded_by_the_original_deadline() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::recovery_mutex_deadline().await;
    let mut config = server.config();
    config.timeout_ms = 400;
    let client = Arc::new(McpClient::connect(&config).await.unwrap());
    client.initialize().await.unwrap();

    let first = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.list_tools().await }
    });
    server.first_stale_request_seen.notified().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.list_tools().await }
    });
    server.recovery_started.notified().await;
    server.release_first_stale_request.notify_one();

    let first_result = tokio::time::timeout(Duration::from_millis(350), first)
        .await
        .expect("recovery mutex wait exceeded the original request deadline")
        .unwrap();
    assert!(matches!(first_result, Err(McpError::Timeout { .. })));
    assert!(matches!(
        second.await.unwrap(),
        Err(McpError::Timeout { .. })
    ));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn captures_a_session_id_only_from_a_successful_initialize() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::failed_initialize_session().await;
    let client = McpClient::connect(&server.config()).await.unwrap();

    assert!(matches!(
        client.initialize().await,
        Err(McpError::ServerError { .. })
    ));
    client.initialize().await.unwrap();
    client.list_tools().await.unwrap();

    assert_eq!(
        server.last_header("initialize", "mcp-session-id").await,
        None
    );
    assert_eq!(
        server.last_header("tools/list", "mcp-session-id").await,
        None
    );
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_empty_or_non_visible_ascii_session_ids_without_installing_them() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");

    for server in [
        FixtureServer::empty_session_id().await,
        FixtureServer::invalid_session_id().await,
    ] {
        let client = McpClient::connect(&server.config()).await.unwrap();
        let error = client.initialize().await.unwrap_err();
        assert!(
            matches!(error, McpError::Protocol(ref message) if message.contains("MCP-Session-Id")),
            "unexpected invalid session error: {error:?}"
        );
        assert_eq!(server.count_method("notifications/initialized").await, 0);

        client.list_tools().await.unwrap();
        assert_eq!(
            server.last_header("tools/list", "mcp-session-id").await,
            None
        );
    }
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_a_streamable_response_with_the_wrong_json_rpc_id() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::wrong_list_id().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    let error = client.list_tools().await.unwrap_err();
    assert!(matches!(error, McpError::Protocol(message) if message.contains("response id")));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn waits_for_the_matching_json_rpc_response_in_an_sse_body() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::unmatched_then_correct_list_id().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(client.list_tools().await.is_ok());
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn server_initiated_request_does_not_consume_the_matching_sse_response() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::server_request_then_response().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(client.list_tools().await.is_ok());
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_sensitive_and_transport_managed_fixed_headers_at_connect() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::json_then_sse().await;
    for name in [
        "Authorization",
        "Proxy-Authorization",
        "Cookie",
        "Set-Cookie",
        "X-Access-Token",
        "X-API-Key",
        "Origin",
        "Mcp-Session-Id",
        "MCP-Protocol-Version",
        "Accept",
        "Content-Type",
        "X-Agent-ID",
        "Last-Event-ID",
    ] {
        let mut config = server.config();
        config
            .headers
            .insert(name.to_string(), "override".to_string());
        let result = McpClient::connect(&config).await;
        assert!(
            matches!(result, Err(McpError::Protocol(message)) if message.contains("header")),
            "header {name} unexpectedly accepted"
        );
    }
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn does_not_follow_cross_origin_redirects_with_streamable_http_headers() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = RedirectFixture::start().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    let result = client.list_tools().await;

    assert!(
        server.redirected_requests.lock().await.is_empty(),
        "cross-origin redirect target received a request"
    );
    assert!(matches!(result, Err(McpError::Http(_))));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn applies_one_timeout_deadline_to_response_headers_and_sse_body() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::timeout_headers_and_body().await;
    let mut config = server.config();
    config.timeout_ms = 80;
    let client = McpClient::connect(&config).await.unwrap();
    client.initialize().await.unwrap();

    let started = tokio::time::Instant::now();
    let error = client.list_tools().await.unwrap_err();
    assert!(matches!(error, McpError::Timeout { .. }));
    assert!(started.elapsed() < Duration::from_millis(110));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn cancels_a_timed_out_non_initialize_request_with_its_original_id() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::never_completes().await;
    let mut config = server.config();
    config.timeout_ms = 80;
    let client = McpClient::connect(&config).await.unwrap();
    client.initialize().await.unwrap();

    let error = client.list_tools().await.unwrap_err();
    assert!(matches!(error, McpError::Timeout { .. }));
    assert_eq!(server.count_method("notifications/cancelled").await, 1);
    assert_eq!(
        server
            .last_body("notifications/cancelled")
            .await
            .and_then(|body| body.pointer("/params/requestId").cloned()),
        Some(json!(2))
    );
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn never_cancels_a_timed_out_initialize_request() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::initialize_never_completes().await;
    let mut config = server.config();
    config.timeout_ms = 80;
    let client = McpClient::connect(&config).await.unwrap();

    assert!(matches!(
        client.initialize().await,
        Err(McpError::Timeout { .. })
    ));
    assert_eq!(server.count_method("notifications/cancelled").await, 0);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn accepts_case_insensitive_sse_content_type() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::uppercase_sse().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();
    assert!(client.list_tools().await.is_ok());
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_notification_success_with_a_body() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::bad_notification().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    let error = client.initialize().await.unwrap_err();
    assert!(matches!(error, McpError::Protocol(message) if message.contains("empty body")));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_notification_response_without_accepted_status() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::wrong_notification_status().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    let error = client.initialize().await.unwrap_err();
    assert!(matches!(error, McpError::Http(message) if message.contains("200")));
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn reconnects_sse_after_eof_with_last_event_id() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::resumable_sse().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(client.list_tools().await.is_ok());
    assert_eq!(server.count_method("GET").await, 1);
    assert_eq!(
        server.last_header("GET", "last-event-id").await.as_deref(),
        Some("event-1")
    );
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn rejects_a_successful_sse_recovery_get_without_event_stream_content_type() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::wrong_recovery_content_type().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    let error = client.list_tools().await.unwrap_err();
    assert!(
        matches!(error, McpError::Protocol(ref message)
            if message.contains("Content-Type") && message.contains("text/event-stream")),
        "unexpected recovery content type error: {error:?}"
    );
    assert_eq!(server.count_method("GET").await, 1);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn empty_sse_event_id_clears_the_reconnect_cursor() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::empty_event_id_resets().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Disconnected)
    ));
    assert_eq!(server.count_method("GET").await, 0);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn sse_retry_delay_is_bounded_by_the_original_request_deadline() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::sse_retry_exceeds_deadline().await;
    let mut config = server.config();
    config.timeout_ms = 80;
    let client = McpClient::connect(&config).await.unwrap();
    client.initialize().await.unwrap();

    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Timeout { .. })
    ));
    assert_eq!(server.count_method("GET").await, 0);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn premature_sse_eof_without_an_event_id_is_a_disconnect_not_a_cancellation() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::disconnect_without_event_id().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Disconnected)
    ));
    assert_eq!(server.count_method("GET").await, 0);
    assert_eq!(server.count_method("notifications/cancelled").await, 0);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn sse_resume_is_bounded_to_one_get_after_premature_eof() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::resume_still_disconnects().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    assert!(matches!(
        client.list_tools().await,
        Err(McpError::Disconnected)
    ));
    assert_eq!(server.count_method("GET").await, 1);
    assert_eq!(server.count_method("notifications/cancelled").await, 0);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}

#[tokio::test]
async fn concurrent_sse_resumes_track_last_event_id_per_post_response() {
    let _environment = ENV_LOCK.lock().await;
    std::env::set_var("MCP_STREAMABLE_HTTP_TEST_TOKEN", "test-token");
    std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
    let server = FixtureServer::concurrent_resumable_sse().await;
    let client = McpClient::connect(&server.config()).await.unwrap();
    client.initialize().await.unwrap();

    let (first, second) = tokio::join!(client.list_tools(), client.list_tools());
    assert!(first.is_ok(), "first request failed: {first:?}");
    assert!(second.is_ok(), "second request failed: {second:?}");
    assert_eq!(server.count_method("GET").await, 2);
    std::env::remove_var("MCP_STREAMABLE_HTTP_TEST_TOKEN");
}
