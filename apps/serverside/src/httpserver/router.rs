use crate::channels::{AdapterResponse, ChannelError, ChannelResult, ChannelRuntime};
use crate::httpserver::channel_ingress::GatewayChannelIngressError;
use crate::httpserver::channel_runtime::{ChannelMessageProcessingError, ChannelRuntimeProcessor};
use crate::httpserver::rate_limit::RateLimitConfig;
use crate::httpserver::sse_sink::{sse_stream_from_receiver, SseLoopEventSink, SseStreamEvent};
use crate::httpserver::GatewayServiceError;
use agent_contracts::InteractionHandle;
use agent_types::interaction::{InteractionRequest, InteractionResponse};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{
        header::{AUTHORIZATION, WWW_AUTHENTICATE},
        HeaderMap, Request, StatusCode,
    },
    middleware::{self, Next},
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tracing::warn;
use xiaoo_shared::gateway::SessionService;

#[derive(Clone)]
pub struct GatewayAppState {
    session_service: Arc<dyn SessionService>,
    session_control_plane: Option<Arc<dyn xiaoo_shared::gateway::SessionControlPlane>>,
    channel_runtimes: Arc<HashMap<String, ChannelRuntime>>,
    channel_processor: ChannelRuntimeProcessor,
    remote_interactions: Arc<RemoteInteractionStore>,
}

impl GatewayAppState {
    pub fn new(session_service: Arc<dyn SessionService>) -> Self {
        Self {
            channel_processor: ChannelRuntimeProcessor::new(session_service.clone()),
            session_service,
            session_control_plane: None,
            channel_runtimes: Arc::new(HashMap::new()),
            remote_interactions: Arc::new(RemoteInteractionStore::default()),
        }
    }

    pub fn with_control_plane(
        session_service: Arc<dyn SessionService>,
        session_control_plane: Arc<dyn xiaoo_shared::gateway::SessionControlPlane>,
    ) -> Self {
        let mut state = Self::new(session_service);
        state.session_control_plane = Some(session_control_plane);
        state
    }

    #[cfg(test)]
    pub(crate) fn with_channel_runtime(
        session_service: Arc<dyn SessionService>,
        runtime: ChannelRuntime,
    ) -> Self {
        let mut runtimes = HashMap::new();
        runtimes.insert(runtime.channel_id.clone(), runtime);
        Self {
            channel_processor: ChannelRuntimeProcessor::new(session_service.clone()),
            session_service,
            session_control_plane: None,
            channel_runtimes: Arc::new(runtimes),
            remote_interactions: Arc::new(RemoteInteractionStore::default()),
        }
    }

    pub fn with_channel_runtimes(
        session_service: Arc<dyn SessionService>,
        runtimes: Vec<ChannelRuntime>,
    ) -> ChannelResult<Self> {
        let mut runtime_map = HashMap::new();
        for runtime in runtimes {
            if runtime_map
                .insert(runtime.channel_id.clone(), runtime)
                .is_some()
            {
                return Err(ChannelError::Config {
                    message: "duplicate channel runtime id".to_string(),
                });
            }
        }
        Ok(Self {
            channel_processor: ChannelRuntimeProcessor::new(session_service.clone()),
            session_service,
            session_control_plane: None,
            channel_runtimes: Arc::new(runtime_map),
            remote_interactions: Arc::new(RemoteInteractionStore::default()),
        })
    }

    pub fn with_channel_runtimes_and_control_plane(
        session_service: Arc<dyn SessionService>,
        session_control_plane: Arc<dyn xiaoo_shared::gateway::SessionControlPlane>,
        runtimes: Vec<ChannelRuntime>,
    ) -> ChannelResult<Self> {
        let mut state = Self::with_channel_runtimes(session_service, runtimes)?;
        state.session_control_plane = Some(session_control_plane);
        Ok(state)
    }

    fn set_channel_interaction_timeout(&mut self, interaction_timeout_secs: u64) {
        self.channel_processor = ChannelRuntimeProcessor::with_timeout(
            self.session_service.clone(),
            interaction_timeout_secs,
        );
    }
}

#[derive(Default)]
struct RemoteInteractionStore {
    pending: Mutex<HashMap<String, oneshot::Sender<InteractionResponse>>>,
}

impl RemoteInteractionStore {
    async fn register(&self, session_id: String) -> oneshot::Receiver<InteractionResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(session_id, tx);
        rx
    }

    async fn answer(&self, session_id: &str, response: InteractionResponse) -> bool {
        self.pending
            .lock()
            .await
            .remove(session_id)
            .map(|tx| tx.send(response).is_ok())
            .unwrap_or(false)
    }
}

struct RemoteSseInteractionHandle {
    session_id: String,
    tx: tokio::sync::mpsc::UnboundedSender<SseStreamEvent>,
    store: Arc<RemoteInteractionStore>,
}

#[async_trait]
impl InteractionHandle for RemoteSseInteractionHandle {
    async fn ask(&self, request: &InteractionRequest) -> InteractionResponse {
        let rx = self.store.register(self.session_id.clone()).await;
        let _ = self.tx.send(SseStreamEvent::InteractionRequested {
            request: request.clone(),
        });

        match rx.await {
            Ok(response) => response,
            Err(_) => default_interaction_response(request),
        }
    }
}

fn default_interaction_response(request: &InteractionRequest) -> InteractionResponse {
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: false },
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: None,
            display_value: None,
        },
        InteractionRequest::Choice { .. } => InteractionResponse::Choice { value: None },
    }
}

#[derive(Debug, Serialize)]
pub struct GatewayHealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct GatewayErrorResponse {
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct HttpBearerAuthConfig {
    token: Arc<str>,
}

impl HttpBearerAuthConfig {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            token: Arc::<str>::from(token.into()),
        }
    }

    fn matches(&self, token: &str) -> bool {
        self.token.as_ref() == token
    }
}

#[cfg(test)]
pub fn create_router_with_auth(
    session_service: Arc<dyn SessionService>,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
) -> Router {
    create_router_from_state(
        GatewayAppState::new(session_service),
        bearer_auth,
        rate_limit,
    )
}

pub fn create_router_with_channel_runtimes_control_plane_and_timeout_and_auth(
    session_service: Arc<dyn SessionService>,
    session_control_plane: Arc<dyn xiaoo_shared::gateway::SessionControlPlane>,
    runtimes: Vec<ChannelRuntime>,
    interaction_timeout_secs: u64,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
) -> ChannelResult<Router> {
    let mut state = GatewayAppState::with_channel_runtimes_and_control_plane(
        session_service,
        session_control_plane,
        runtimes,
    )?;
    state.set_channel_interaction_timeout(interaction_timeout_secs);
    Ok(create_router_from_state(state, bearer_auth, rate_limit))
}

fn create_router_from_state(
    state: GatewayAppState,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
) -> Router {
    let protected_runtime_routes = apply_http_bearer_auth(
        Router::new()
            .route("/api/v1/runtimes/open", post(handle_session_open))
            .route("/api/v1/runtimes/input", post(handle_session_input))
            .route(
                "/api/v1/runtimes/interaction",
                post(handle_session_interaction),
            )
            .route("/api/v1/runtimes/cancel", post(handle_session_cancel))
            .route("/api/v1/runtimes/close", post(handle_session_close))
            .route(
                "/api/v1/runtimes/checkpoint",
                post(handle_runtime_checkpoint),
            )
            .route(
                "/api/v1/runtimes/checkpoint/delete-snapshot",
                post(handle_runtime_checkpoint_snapshot_delete),
            )
            .route("/api/v1/runtimes/pause", post(handle_runtime_pause))
            .route("/api/v1/runtimes/resume", post(handle_runtime_resume))
            .route("/api/v1/runtimes/checkout", post(handle_runtime_checkout))
            .route("/api/v1/runtimes/exec", post(handle_runtime_exec))
            .route("/api/v1/runtimes/read-file", post(handle_runtime_read_file))
            .route(
                "/api/v1/runtimes/write-file",
                post(handle_runtime_write_file),
            ),
        bearer_auth.clone(),
    );

    let router = Router::new()
        .route("/api/v1/health", get(health_check))
        .route(
            "/api/v1/channels/:channel_id/events",
            post(handle_channel_events),
        )
        .merge(protected_runtime_routes)
        .with_state(Arc::new(state));

    match rate_limit.and_then(|c| c.governor_layer()) {
        Some(layer) => router.layer(layer),
        None => router,
    }
}

pub fn create_router_with_control_plane_and_auth(
    session_service: Arc<dyn SessionService>,
    session_control_plane: Arc<dyn xiaoo_shared::gateway::SessionControlPlane>,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
) -> Router {
    create_router_from_state(
        GatewayAppState::with_control_plane(session_service, session_control_plane),
        bearer_auth,
        rate_limit,
    )
}

fn apply_http_bearer_auth<S>(
    router: Router<S>,
    bearer_auth: Option<HttpBearerAuthConfig>,
) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    match bearer_auth {
        Some(bearer_auth) => router.route_layer(middleware::from_fn_with_state(
            bearer_auth,
            require_bearer_auth,
        )),
        None => router,
    }
}

async fn require_bearer_auth(
    State(auth): State<HttpBearerAuthConfig>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let token = match parse_bearer_token(request.headers()) {
        Ok(token) => token,
        Err(error) => return unauthorized_response(error),
    };

    if !auth.matches(token) {
        return unauthorized_response("invalid bearer token");
    }

    next.run(request).await
}

fn parse_bearer_token(headers: &HeaderMap) -> Result<&str, &'static str> {
    let value = headers
        .get(AUTHORIZATION)
        .ok_or("missing bearer token")?
        .to_str()
        .map_err(|_| "invalid authorization header")?;
    let mut parts = value.split_whitespace();
    let scheme = parts.next().ok_or("missing bearer token")?;
    let token = parts.next().ok_or("missing bearer token")?;
    if parts.next().is_some() {
        return Err("invalid authorization header");
    }
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err("invalid authorization scheme");
    }
    if token.is_empty() {
        return Err("missing bearer token");
    }
    Ok(token)
}

fn unauthorized_response(message: impl Into<String>) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(WWW_AUTHENTICATE, "Bearer")],
        Json(GatewayErrorResponse {
            error: message.into(),
        }),
    )
        .into_response()
}

async fn health_check() -> Json<GatewayHealthResponse> {
    Json(GatewayHealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn handle_session_open(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeOpenRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.open_session(payload).await {
        Ok(record) => Json(record).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_session_input(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeTurnRequest>,
) -> Response {
    stream_session_input(state, payload.session_id.clone(), payload).await
}

async fn stream_session_input(
    state: Arc<GatewayAppState>,
    session_id: String,
    payload: xiaoo_shared::gateway::RuntimeTurnRequest,
) -> Response {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SseStreamEvent>();
    let sink = Arc::new(SseLoopEventSink::new(tx.clone()));
    let interaction_handle = Arc::new(RemoteSseInteractionHandle {
        session_id: session_id.clone(),
        tx: tx.clone(),
        store: state.remote_interactions.clone(),
    });
    let session_service = state.session_service.clone();
    let conversation_id = payload.conversation_id.clone();

    tokio::spawn(async move {
        match session_service
            .run_turn_with_interaction(payload, Some(sink.clone()), Some(interaction_handle), None)
            .await
        {
            Ok(result) => {
                let summary = sink.take_loop_summary();
                let filtered_messages = filter_messages_for_display(&result.messages);
                let _ = tx.send(SseStreamEvent::Done {
                    reply: result.visible_reply.clone(),
                    raw_reply: result.raw_reply,
                    conversation_id,
                    session_id,
                    turn_count: summary.as_ref().map_or(0, |s| s.turn_count),
                    total_tokens: result.total_tokens as usize,
                    prompt_tokens: result.prompt_tokens,
                    completion_tokens: result.completion_tokens,
                    estimated_input_tokens: result.estimated_input_tokens,
                    messages: filtered_messages,
                    stop_reason: summary.map(|s| s.stop_reason).unwrap_or_default(),
                });
            }
            Err(error) => {
                let _ = tx.send(SseStreamEvent::Error {
                    error: error.to_string(),
                });
            }
        }
    });

    Sse::new(sse_stream_from_receiver(rx))
        .keep_alive(KeepAlive::default())
        .into_response()
}

async fn handle_session_interaction(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeInteractionRequest>,
) -> Response {
    if state
        .remote_interactions
        .answer(&payload.session_id, payload.response)
        .await
    {
        StatusCode::NO_CONTENT.into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(GatewayErrorResponse {
                error: "no pending interaction for session".to_string(),
            }),
        )
            .into_response()
    }
}

async fn handle_session_cancel(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeCancelRequest>,
) -> Response {
    let session_id = payload.session_id;
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return Json(SseStreamEvent::Cancelled { session_id }).into_response();
    };

    match control_plane.resume_session(&session_id).await {
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(GatewayErrorResponse {
                error: format!("session not found: {}", session_id),
            }),
        )
            .into_response(),
        Ok(Some(_)) => match control_plane
            .submit_input(
                &session_id,
                xiaoo_shared::gateway::SessionInput::CancelActiveTurn,
            )
            .await
        {
            Ok(_) => Json(SseStreamEvent::Cancelled { session_id }).into_response(),
            Err(error) => map_session_error(error),
        },
        Err(error) => map_session_error(error),
    }
}

async fn handle_session_close(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeCloseRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.force_close_session(&payload.session_id).await {
        Ok(record) => Json(record).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_checkpoint(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeCheckpointRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.checkpoint_runtime(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_checkout(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeCheckoutRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.checkout_runtime(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_pause(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimePauseRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.pause_runtime(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_resume(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeResumeRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.resume_runtime(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_checkpoint_snapshot_delete(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeCheckpointSnapshotDeleteRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.delete_checkpoint_snapshot(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_exec(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeExecRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.exec_runtime(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_read_file(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeReadFileRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.read_runtime_file(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_write_file(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::RuntimeWriteFileRequest>,
) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.write_runtime_file(payload).await {
        Ok(result) => Json(result).into_response(),
        Err(error) => map_session_error(error),
    }
}

fn map_session_error(error: xiaoo_shared::gateway::SessionServiceError) -> Response {
    let status = match &error {
        xiaoo_shared::gateway::SessionServiceError::SessionNotFound { .. } => StatusCode::NOT_FOUND,
        xiaoo_shared::gateway::SessionServiceError::SessionBusy { .. } => {
            StatusCode::TOO_MANY_REQUESTS
        }
        xiaoo_shared::gateway::SessionServiceError::SessionClosed { .. } => StatusCode::CONFLICT,
        xiaoo_shared::gateway::SessionServiceError::UnsupportedCapability { .. } => {
            StatusCode::NOT_IMPLEMENTED
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (
        status,
        Json(GatewayErrorResponse {
            error: error.to_string(),
        }),
    )
        .into_response()
}

async fn handle_channel_events(
    State(state): State<Arc<GatewayAppState>>,
    Path(channel_id): Path<String>,
    Query(query): Query<HashMap<String, String>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(runtime) = state.channel_runtimes.get(&channel_id).cloned() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GatewayErrorResponse {
                error: format!("{channel_id} webhook is not configured"),
            }),
        )
            .into_response();
    };

    let adapter = runtime.adapter.clone();

    match adapter.handle_event(&headers, &query, body.as_ref()).await {
        Ok((AdapterResponse::Challenge { challenge }, _)) => {
            Json(serde_json::json!({ "challenge": challenge })).into_response()
        }
        Ok((adapter_response, maybe_message)) => {
            if let Some(message) = maybe_message {
                if runtime.capabilities.supports_reactions {
                    if let Err(error) = runtime
                        .adapter
                        .acknowledge_message(&message.message_id)
                        .await
                    {
                        warn!(
                            "failed to acknowledge channel message: channel={} id={} conversation={} error={}",
                            runtime.meta.id, message.message_id, message.conversation_id, error
                        );
                    }
                }
                if runtime.capabilities.requires_async_processing {
                    let processor = state.channel_processor.clone();
                    let runtime = runtime.clone();
                    tokio::spawn(async move {
                        if let Err(error) = processor.process_message(runtime, message).await {
                            warn!("failed to process async channel message: {error}");
                        }
                    });
                } else if let Err(error) = state
                    .channel_processor
                    .process_message(runtime.clone(), message)
                    .await
                {
                    return map_channel_message_processing_error(error);
                }
            }
            map_adapter_response(adapter_response)
        }
        Err(error) => map_channel_error(error),
    }
}

fn map_adapter_response(adapter_response: AdapterResponse) -> Response {
    match adapter_response {
        AdapterResponse::Accepted => {
            Json(serde_json::json!({ "code": 0, "message": "ok" })).into_response()
        }
        AdapterResponse::CustomJson { body } => Json(body).into_response(),
        AdapterResponse::Challenge { .. } => {
            unreachable!("challenge responses are handled before adapter mapping")
        }
    }
}

fn map_channel_ingress_error(error: GatewayChannelIngressError) -> Response {
    let status = match error {
        GatewayChannelIngressError::UnsupportedAttachments => StatusCode::NOT_IMPLEMENTED,
    };
    (
        status,
        Json(GatewayErrorResponse {
            error: error.to_string(),
        }),
    )
        .into_response()
}

fn map_channel_error(error: ChannelError) -> Response {
    let status = match error {
        ChannelError::Config { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        ChannelError::InvalidEvent { .. } => StatusCode::BAD_REQUEST,
        ChannelError::Authentication { .. } => StatusCode::UNAUTHORIZED,
        ChannelError::Transport { .. } | ChannelError::Delivery { .. } => StatusCode::BAD_GATEWAY,
        ChannelError::UnsupportedCapability { .. } => StatusCode::NOT_IMPLEMENTED,
    };

    (
        status,
        Json(GatewayErrorResponse {
            error: error.to_string(),
        }),
    )
        .into_response()
}

fn map_gateway_error(error: GatewayServiceError) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(GatewayErrorResponse {
            error: error.to_string(),
        }),
    )
        .into_response()
}

fn map_channel_message_processing_error(error: ChannelMessageProcessingError) -> Response {
    match error {
        ChannelMessageProcessingError::ChannelIngress(error) => map_channel_ingress_error(error),
        ChannelMessageProcessingError::Gateway(error) => map_gateway_error(error),
        ChannelMessageProcessingError::Channel(error) => map_channel_error(error),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create_router_with_auth, handle_channel_events, GatewayAppState, GatewayErrorResponse,
        HttpBearerAuthConfig,
    };
    use crate::channels::{
        AdapterResponse, ChannelAdapter, ChannelCapabilities, ChannelMember, ChannelMention,
        ChannelMessage, ChannelMeta, ChannelResult, ChannelRuntime, ChannelTextFormat,
    };
    use agent_contracts::LoopEventSink;
    use async_trait::async_trait;
    use axum::{
        body::{to_bytes, Body, Bytes},
        extract::{Path, Query, State},
        http::{HeaderMap, Request, StatusCode},
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::time::{sleep, timeout, Duration};
    use tower::util::ServiceExt;
    use xiaoo_shared::gateway::{
        AppTurnRequest, AppTurnResult, SessionService, SessionServiceError, TurnOutcome,
    };

    #[tokio::test(flavor = "current_thread")]
    async fn bearer_auth_rejects_missing_token_for_runtime_input() {
        let router = create_router_with_auth(
            Arc::new(FakeSessionService::new("unused")),
            Some(HttpBearerAuthConfig::new("secret-token")),
            None,
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/input")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"runtime_id":"runtime-1","entry":{"kind":"tui"},"channel":"tui","conversation_id":"conv-1","sender_id":"user-1","text":"hello","mentions":[]}"#,
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response
                .headers()
                .get("www-authenticate")
                .and_then(|h| h.to_str().ok()),
            Some("Bearer")
        );

        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body should read");
        let payload: GatewayErrorResponse =
            serde_json::from_slice(&body).expect("error response should parse");
        assert_eq!(payload.error, "missing bearer token");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bearer_auth_allows_valid_token_for_runtime_input() {
        let router = create_router_with_auth(
            Arc::new(FakeSessionService::new("unused")),
            Some(HttpBearerAuthConfig::new("secret-token")),
            None,
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/input")
                    .header("authorization", "Bearer secret-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"runtime_id":"runtime-1","entry":{"kind":"tui"},"channel":"tui","conversation_id":"conv-1","sender_id":"user-1","text":"hello","mentions":[]}"#,
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bearer_auth_applies_to_runtime_checkpoint_route() {
        let router = create_router_with_auth(
            Arc::new(FakeSessionService::new("unused")),
            Some(HttpBearerAuthConfig::new("secret-token")),
            None,
        );

        let missing_auth = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/checkpoint")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"runtime_id":"runtime-1"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(missing_auth.status(), StatusCode::UNAUTHORIZED);

        let valid_auth = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/checkpoint")
                    .header("authorization", "Bearer secret-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"runtime_id":"runtime-1"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(valid_auth.status(), StatusCode::NOT_IMPLEMENTED);

        let missing_auth_delete_snapshot = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/checkpoint/delete-snapshot")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"checkpoint_id":"rtcp_demo"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(
            missing_auth_delete_snapshot.status(),
            StatusCode::UNAUTHORIZED
        );

        let valid_auth_delete_snapshot = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/checkpoint/delete-snapshot")
                    .header("authorization", "Bearer secret-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"checkpoint_id":"rtcp_demo"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");
        assert_eq!(
            valid_auth_delete_snapshot.status(),
            StatusCode::NOT_IMPLEMENTED
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn runtime_close_uses_body_runtime_id_route() {
        let router = create_router_with_auth(
            Arc::new(FakeSessionService::new("unused")),
            Some(HttpBearerAuthConfig::new("secret-token")),
            None,
        );

        let response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/runtimes/close")
                    .header("authorization", "Bearer secret-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"runtime_id":"runtime-1"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn old_session_control_plane_routes_are_not_registered() {
        let router = create_router_with_auth(
            Arc::new(FakeSessionService::new("unused")),
            Some(HttpBearerAuthConfig::new("secret-token")),
            None,
        );

        let input_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/input")
                    .header("authorization", "Bearer secret-token")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"session_id":"session-1","entry":{"kind":"tui"},"channel":"tui","conversation_id":"conv-1","sender_id":"user-1","text":"hello","mentions":[]}"#,
                    ))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(input_response.status(), StatusCode::NOT_FOUND);

        let close_response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/sessions/close")
                    .header("authorization", "Bearer secret-token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"runtime_id":"runtime-1"}"#))
                    .expect("request should build"),
            )
            .await
            .expect("router should respond");

        assert_eq!(close_response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn bearer_auth_does_not_apply_to_health_or_feishu_webhook() {
        let router = create_router_with_auth(
            Arc::new(FakeSessionService::new("unused")),
            Some(HttpBearerAuthConfig::new("secret-token")),
            None,
        );

        let health_response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/api/v1/health")
                    .body(Body::empty())
                    .expect("request should build"),
            )
            .await
            .expect("health route should respond");
        assert_eq!(health_response.status(), StatusCode::OK);

        let feishu_response = router
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/v1/channels/feishu/events")
                    .body(Body::from("{}"))
                    .expect("request should build"),
            )
            .await
            .expect("feishu route should respond");
        assert_eq!(feishu_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    struct FakeSessionService {
        requests: Mutex<Vec<AppTurnRequest>>,
        reply: String,
        delay: Duration,
    }

    impl FakeSessionService {
        fn new(reply: impl Into<String>) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                reply: reply.into(),
                delay: Duration::ZERO,
            }
        }

        fn with_delay(reply: impl Into<String>, delay: Duration) -> Self {
            Self {
                requests: Mutex::new(Vec::new()),
                reply: reply.into(),
                delay,
            }
        }

        async fn run_turn_impl(
            &self,
            request: AppTurnRequest,
        ) -> Result<AppTurnResult, SessionServiceError> {
            if !self.delay.is_zero() {
                sleep(self.delay).await;
            }
            self.requests
                .lock()
                .expect("session service mutex poisoned")
                .push(request);
            Ok(AppTurnResult {
                raw_reply: self.reply.clone(),
                visible_reply: self.reply.clone(),
                messages: Vec::new(),
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
                estimated_input_tokens: 0,
                outcome: TurnOutcome::Complete,
            })
        }
    }

    #[async_trait]
    impl SessionService for FakeSessionService {
        async fn run_turn(
            &self,
            request: AppTurnRequest,
        ) -> Result<AppTurnResult, SessionServiceError> {
            self.run_turn_impl(request).await
        }

        async fn run_turn_with_events(
            &self,
            request: AppTurnRequest,
            _event_sink: Option<Arc<dyn LoopEventSink>>,
        ) -> Result<AppTurnResult, SessionServiceError> {
            self.run_turn_impl(request).await
        }
    }

    struct FakeChannelAdapter {
        event_result: ChannelResult<(AdapterResponse, Option<ChannelMessage>)>,
        sent_texts: Mutex<Vec<(String, String, Option<String>)>>,
        listed_members: Vec<ChannelMember>,
    }

    #[async_trait]
    impl ChannelAdapter for FakeChannelAdapter {
        fn channel_name(&self) -> &str {
            "feishu"
        }

        async fn handle_event(
            &self,
            _headers: &HeaderMap,
            _query: &HashMap<String, String>,
            _body: &[u8],
        ) -> ChannelResult<(AdapterResponse, Option<ChannelMessage>)> {
            self.event_result.clone()
        }

        async fn send_text(
            &self,
            conversation_id: &str,
            text: &str,
            reply_to_message_id: Option<&str>,
        ) -> ChannelResult<Option<String>> {
            self.sent_texts
                .lock()
                .expect("channel adapter mutex poisoned")
                .push((
                    conversation_id.to_string(),
                    text.to_string(),
                    reply_to_message_id.map(|value| value.to_string()),
                ));
            Ok(Some("om_reply".to_string()))
        }

        async fn list_members(&self, _conversation_id: &str) -> ChannelResult<Vec<ChannelMember>> {
            Ok(self.listed_members.clone())
        }
    }

    fn build_fake_runtime(
        adapter: Arc<dyn ChannelAdapter>,
        requires_async_processing: bool,
    ) -> ChannelRuntime {
        ChannelRuntime {
            instance_id: "ops-feishu".to_string(),
            channel_id: "feishu".to_string(),
            meta: ChannelMeta {
                id: "feishu".to_string(),
                label: "Feishu".to_string(),
                selection_label: "Feishu".to_string(),
                docs_path: "/channels/feishu".to_string(),
                docs_label: "feishu".to_string(),
                blurb: "test".to_string(),
                aliases: Vec::new(),
                order: 0,
            },
            capabilities: ChannelCapabilities {
                supports_webhook: true,
                supports_direct_messages: true,
                supports_group_messages: true,
                requires_async_processing,
                supports_threads: true,
                supports_media: false,
                supports_member_listing: true,
                supports_reactions: false,
                supports_progress_updates: false,
                text_reply_format: ChannelTextFormat::PlainText,
            },
            adapter,
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn feishu_route_processes_adapter_message_and_replies() {
        let session_service = Arc::new(FakeSessionService::new("处理完成"));
        let fake_adapter = Arc::new(FakeChannelAdapter {
            event_result: Ok((
                AdapterResponse::Accepted,
                Some(ChannelMessage {
                    channel: "feishu".to_string(),
                    channel_instance_id: Some("ops-feishu".to_string()),
                    conversation_id: "conv-1".to_string(),
                    sender_id: "user-1".to_string(),
                    message_id: "msg-1".to_string(),
                    text: "hello".to_string(),
                    reply_to_message_id: Some("parent-1".to_string()),
                    root_message_id: None,
                    mentions: vec![ChannelMention {
                        id: "bot".to_string(),
                        display_name: Some("XiaoO".to_string()),
                    }],
                    attachments: Vec::new(),
                }),
            )),
            sent_texts: Mutex::new(Vec::new()),
            listed_members: vec![
                ChannelMember {
                    id: "user-2".to_string(),
                    display_name: Some("陈卓".to_string()),
                },
                ChannelMember {
                    id: "user-3".to_string(),
                    display_name: Some("罗一鸣".to_string()),
                },
            ],
        });
        let runtime = build_fake_runtime(fake_adapter.clone(), false);
        let state = Arc::new(GatewayAppState::with_channel_runtime(
            session_service.clone(),
            runtime,
        ));

        let response = handle_channel_events(
            State(state),
            Path("feishu".to_string()),
            Query(HashMap::new()),
            HeaderMap::new(),
            Bytes::from_static(b"{}"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let requests = session_service
            .requests
            .lock()
            .expect("session service mutex poisoned");
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].session_id, "ops-feishu:conv-1");
        let identity_prompt = requests[0]
            .channel_identity_prompt
            .as_deref()
            .expect("channel identity prompt should be present");
        assert!(identity_prompt.contains("<person uid=\"user-1\">user-1</person>"));
        assert!(identity_prompt.contains("<person uid=\"user-2\">陈卓</person>"));
        assert!(identity_prompt.contains("<person uid=\"user-3\">罗一鸣</person>"));
        drop(requests);

        let sent_texts = fake_adapter
            .sent_texts
            .lock()
            .expect("channel adapter mutex poisoned");
        assert_eq!(sent_texts.len(), 1);
        assert_eq!(sent_texts[0].0, "conv-1");
        assert_eq!(sent_texts[0].1, "处理完成");
        assert_eq!(sent_texts[0].2.as_deref(), Some("parent-1"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn feishu_route_returns_challenge_without_running_session() {
        let session_service = Arc::new(FakeSessionService::new("unused"));
        let adapter: Arc<dyn ChannelAdapter> = Arc::new(FakeChannelAdapter {
            event_result: Ok((
                AdapterResponse::Challenge {
                    challenge: "challenge-token".to_string(),
                },
                None,
            )),
            sent_texts: Mutex::new(Vec::new()),
            listed_members: Vec::new(),
        });
        let state = Arc::new(GatewayAppState::with_channel_runtime(
            session_service.clone(),
            build_fake_runtime(adapter, false),
        ));

        let response = handle_channel_events(
            State(state),
            Path("feishu".to_string()),
            Query(HashMap::new()),
            HeaderMap::new(),
            Bytes::from_static(b"{}"),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(session_service
            .requests
            .lock()
            .expect("session service mutex poisoned")
            .is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn async_channel_route_returns_ack_before_turn_completes() {
        let session_service = Arc::new(FakeSessionService::with_delay(
            "处理完成",
            Duration::from_millis(200),
        ));
        let fake_adapter = Arc::new(FakeChannelAdapter {
            event_result: Ok((
                AdapterResponse::Accepted,
                Some(ChannelMessage {
                    channel: "feishu".to_string(),
                    channel_instance_id: Some("ops-feishu".to_string()),
                    conversation_id: "conv-1".to_string(),
                    sender_id: "user-1".to_string(),
                    message_id: "msg-1".to_string(),
                    text: "hello".to_string(),
                    reply_to_message_id: Some("parent-1".to_string()),
                    root_message_id: None,
                    mentions: Vec::new(),
                    attachments: Vec::new(),
                }),
            )),
            sent_texts: Mutex::new(Vec::new()),
            listed_members: vec![ChannelMember {
                id: "user-2".to_string(),
                display_name: Some("陈卓".to_string()),
            }],
        });
        let state = Arc::new(GatewayAppState::with_channel_runtime(
            session_service.clone(),
            build_fake_runtime(fake_adapter.clone(), true),
        ));

        let response = timeout(
            Duration::from_millis(50),
            handle_channel_events(
                State(state),
                Path("feishu".to_string()),
                Query(HashMap::new()),
                HeaderMap::new(),
                Bytes::from_static(b"{}"),
            ),
        )
        .await
        .expect("async webhook route should acknowledge immediately");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(session_service
            .requests
            .lock()
            .expect("session service mutex poisoned")
            .is_empty());

        sleep(Duration::from_millis(250)).await;

        let requests = session_service
            .requests
            .lock()
            .expect("session service mutex poisoned");
        assert_eq!(requests.len(), 1);
        drop(requests);

        let sent_texts = fake_adapter
            .sent_texts
            .lock()
            .expect("channel adapter mutex poisoned");
        assert_eq!(sent_texts.len(), 1);
        assert_eq!(sent_texts[0].1, "处理完成");
    }
}

fn filter_messages_for_display(
    messages: &[llm_client::ChatMessage],
) -> Vec<llm_client::ChatMessage> {
    messages.iter().map(filter_message_for_display).collect()
}

fn filter_message_for_display(message: &llm_client::ChatMessage) -> llm_client::ChatMessage {
    use agent_types::llm::ContentBlock;
    let filtered_blocks: Vec<ContentBlock> = message
        .blocks
        .iter()
        .map(|block| match block {
            ContentBlock::ToolResult {
                call_id,
                tool_name,
                output,
                is_error,
            } => {
                if tool_name == "ask_user_question" {
                    let filtered_output = filter_ask_user_question_output(output);
                    ContentBlock::ToolResult {
                        call_id: call_id.clone(),
                        tool_name: tool_name.clone(),
                        output: filtered_output,
                        is_error: *is_error,
                    }
                } else {
                    block.clone()
                }
            }
            _ => block.clone(),
        })
        .collect();

    llm_client::ChatMessage {
        role: message.role.clone(),
        blocks: filtered_blocks,
        message_id: message.message_id.clone(),
        timestamp_ms: message.timestamp_ms,
        api_usage_tokens: message.api_usage_tokens,
        reasoning_content: message.reasoning_content.clone(),
        estimated_tokens: message.estimated_tokens,
    }
}

fn filter_ask_user_question_output(output: &str) -> String {
    if let Ok(mut json_value) = serde_json::from_str::<serde_json::Value>(output) {
        if let Some(answers) = json_value.get_mut("answers") {
            if let Some(answers_array) = answers.as_array_mut() {
                for answer in answers_array {
                    if let Some(kind) = answer.get("kind") {
                        if kind.as_str() == Some("text") {
                            let display_value = answer.get("display_value").and_then(|v| {
                                if v.is_null() {
                                    None
                                } else {
                                    Some(v.clone())
                                }
                            });
                            if let Some(display_val) = display_value {
                                if let Some(obj) = answer.as_object_mut() {
                                    obj["value"] = display_val;
                                    obj.remove("display_value");
                                }
                            }
                        }
                    }
                }
            }
        }
        if let Ok(filtered_output) = serde_json::to_string(&json_value) {
            return filtered_output;
        }
    }
    output.to_string()
}
