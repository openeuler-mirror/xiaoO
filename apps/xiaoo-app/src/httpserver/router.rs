use crate::channels::feishu::{FeishuAdapter, FeishuConfig};
use crate::channels::{
    feishu_capabilities, feishu_meta, AdapterResponse, ChannelAdapter, ChannelError, ChannelResult,
    ChannelRuntime,
};
use crate::gateway::channel_interaction::{
    resolve_interaction_from_text, ChannelInteractionHandle,
};
use crate::gateway::pending_interaction::PendingInteractionStore;
use crate::gateway::{channel_session_id, ChannelProgressRelayHandle, SessionService};
use crate::httpserver::channel_ingress::{
    build_gateway_channel_message, GatewayChannelIngressError, GatewayChannelMention,
    GatewayChannelMessage,
};
use crate::httpserver::sse_sink::{sse_stream_from_receiver, SseLoopEventSink, SseStreamEvent};
use crate::httpserver::{GatewayService, GatewayServiceError};
use agent_contracts::LoopEventSink;
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{
        sse::{KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use thiserror::Error;
use tracing::warn;

#[derive(Clone)]
pub struct GatewayAppState {
    gateway_service: Arc<GatewayService>,
    feishu_runtime: Option<ChannelRuntime>,
    pending_interactions: Arc<PendingInteractionStore>,
    interaction_timeout_secs: u64,
}

impl GatewayAppState {
    pub fn new(session_service: Arc<dyn SessionService>) -> Self {
        let pending_interactions = Arc::new(PendingInteractionStore::new());
        Self {
            gateway_service: Arc::new(GatewayService::new(session_service)),
            feishu_runtime: None,
            pending_interactions,
            interaction_timeout_secs: 120,
        }
    }

    pub fn with_feishu(
        session_service: Arc<dyn SessionService>,
        feishu_config: FeishuConfig,
    ) -> ChannelResult<Self> {
        let adapter: Arc<dyn ChannelAdapter> = Arc::new(FeishuAdapter::new(feishu_config)?);
        Ok(Self::with_channel_runtime(
            session_service,
            ChannelRuntime {
                instance_id: "feishu".to_string(),
                channel_id: "feishu".to_string(),
                meta: feishu_meta(),
                capabilities: feishu_capabilities(),
                adapter,
            },
        ))
    }

    pub(crate) fn with_channel_runtime(
        session_service: Arc<dyn SessionService>,
        runtime: ChannelRuntime,
    ) -> Self {
        Self {
            gateway_service: Arc::new(GatewayService::new(session_service)),
            feishu_runtime: Some(runtime),
            pending_interactions: Arc::new(PendingInteractionStore::new()),
            interaction_timeout_secs: 120,
        }
    }
}

#[derive(Debug, Error)]
enum ChannelMessageProcessingError {
    #[error(transparent)]
    ChannelIngress(#[from] GatewayChannelIngressError),
    #[error(transparent)]
    Gateway(#[from] GatewayServiceError),
    #[error(transparent)]
    Channel(#[from] ChannelError),
}

#[derive(Debug, Serialize)]
pub struct GatewayHealthResponse {
    pub status: &'static str,
    pub version: &'static str,
}

#[derive(Debug, Serialize)]
pub struct GatewayErrorResponse {
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct TestChatTurnRequest {
    pub text: String,
    pub channel: String,
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    pub sender_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    pub conversation_id: String,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub root_message_id: Option<String>,
    #[serde(default)]
    pub mentions: Vec<TestChatMention>,
}

#[derive(Debug, Deserialize)]
pub struct TestChatRequest {
    pub text: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub channel_instance_id: Option<String>,
    #[serde(default)]
    pub sender_id: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub message_id: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub root_message_id: Option<String>,
    #[serde(default)]
    pub mentions: Vec<TestChatMention>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TestChatMention {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct TestChatResponse {
    pub reply: String,
    pub raw_reply: String,
    pub conversation_id: String,
    pub session_id: String,
}

pub fn create_router(session_service: Arc<dyn SessionService>) -> Router {
    create_router_from_state(GatewayAppState::new(session_service))
}

pub fn create_router_with_feishu_and_timeout(
    session_service: Arc<dyn SessionService>,
    feishu_config: FeishuConfig,
    interaction_timeout_secs: u64,
) -> ChannelResult<Router> {
    let mut state = GatewayAppState::with_feishu(session_service, feishu_config)?;
    state.interaction_timeout_secs = interaction_timeout_secs;
    Ok(create_router_from_state(state))
}

fn create_router_from_state(state: GatewayAppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health_check))
        .route("/api/v1/chat", post(handle_chat))
        .route("/api/v1/chat/stream", post(handle_chat_stream))
        .route("/api/v1/channels/feishu/events", post(handle_feishu_events))
        .with_state(Arc::new(state))
}

async fn health_check() -> Json<GatewayHealthResponse> {
    Json(GatewayHealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn handle_chat(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<TestChatRequest>,
) -> Response {
    let request = match validate_test_chat_request(payload) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(GatewayErrorResponse { error }),
            )
                .into_response();
        }
    };

    let message = GatewayChannelMessage {
        channel: request.channel,
        channel_instance_id: request.channel_instance_id,
        conversation_id: request.conversation_id,
        sender_id: request.sender_id,
        agent_preset_id: request.agent,
        message_id: request
            .message_id
            .unwrap_or_else(|| format!("test-msg-{}", uuid::Uuid::new_v4())),
        text: request.text,
        channel_identity_prompt: None,
        reply_to_message_id: request.reply_to_message_id,
        root_message_id: request.root_message_id,
        mentions: request
            .mentions
            .into_iter()
            .map(|mention| GatewayChannelMention {
                id: mention.id,
                display_name: mention.display_name,
            })
            .collect(),
    };

    match state.gateway_service.handle_channel_message(message).await {
        Ok(response) => Json(TestChatResponse {
            reply: response.visible_reply,
            raw_reply: response.raw_reply,
            conversation_id: response.conversation_id,
            session_id: response.session_id,
        })
        .into_response(),
        Err(error) => map_gateway_error(error),
    }
}

async fn handle_chat_stream(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<TestChatRequest>,
) -> Response {
    let request = match validate_test_chat_request(payload) {
        Ok(request) => request,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(GatewayErrorResponse { error }),
            )
                .into_response();
        }
    };

    let conversation_id = request.conversation_id.clone();
    let message = GatewayChannelMessage {
        channel: request.channel,
        channel_instance_id: request.channel_instance_id,
        conversation_id,
        sender_id: request.sender_id,
        agent_preset_id: request.agent,
        message_id: request
            .message_id
            .unwrap_or_else(|| format!("test-msg-{}", uuid::Uuid::new_v4())),
        text: request.text,
        channel_identity_prompt: None,
        reply_to_message_id: request.reply_to_message_id,
        root_message_id: request.root_message_id,
        mentions: request
            .mentions
            .into_iter()
            .map(|mention| GatewayChannelMention {
                id: mention.id,
                display_name: mention.display_name,
            })
            .collect(),
    };

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SseStreamEvent>();
    let sink = Arc::new(SseLoopEventSink::new(tx.clone()));

    tokio::spawn(async move {
        match state
            .gateway_service
            .handle_channel_message_with_interaction(message, Some(sink.clone()), None)
            .await
        {
            Ok(response) => {
                let summary = sink.take_loop_summary();
                let _ = tx.send(SseStreamEvent::Done {
                    reply: response.visible_reply,
                    raw_reply: response.raw_reply,
                    conversation_id: response.conversation_id,
                    session_id: response.session_id,
                    turn_count: summary.as_ref().map_or(0, |s| s.turn_count),
                    total_tokens: summary.as_ref().map_or(0, |s| s.total_tokens),
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

async fn handle_feishu_events(
    State(state): State<Arc<GatewayAppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(runtime) = state.feishu_runtime.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GatewayErrorResponse {
                error: "feishu webhook is not configured".to_string(),
            }),
        )
            .into_response();
    };

    let adapter = runtime.adapter.clone();

    match adapter.handle_event(&headers, body.as_ref()).await {
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
                            runtime.meta.id,
                            message.message_id,
                            message.conversation_id,
                            error
                        );
                    }
                }
                if runtime.capabilities.requires_async_processing {
                    let state = state.clone();
                    let runtime = runtime.clone();
                    tokio::spawn(async move {
                        if let Err(error) = process_channel_message(state, runtime, message).await {
                            warn!("failed to process async channel message: {error}");
                        }
                    });
                } else if let Err(error) =
                    process_channel_message(state.clone(), runtime.clone(), message).await
                {
                    return map_channel_message_processing_error(error);
                }
            }
            map_adapter_response(adapter_response)
        }
        Err(error) => map_channel_error(error),
    }
}

async fn process_channel_message(
    state: Arc<GatewayAppState>,
    runtime: ChannelRuntime,
    message: crate::channels::ChannelMessage,
) -> Result<(), ChannelMessageProcessingError> {
    let adapter = runtime.adapter.clone();
    let conversation_id = message.conversation_id.clone();
    let reply_to_message_id = message.reply_to_message_id.clone();

    // --- Check for pending ask_user_question interaction ---
    let session_id = channel_session_id(
        &runtime.channel_id,
        Some(&runtime.instance_id),
        &conversation_id,
    );
    if let Some(pending) = state.pending_interactions.take(&session_id).await {
        let response = resolve_interaction_from_text(&message.text, &pending.request);
        let _ = pending.response_tx.send(response);
        return Ok(());
    }
    let progress_relay = runtime.capabilities.supports_progress_updates.then(|| {
        ChannelProgressRelayHandle::new(
            adapter.clone(),
            conversation_id.clone(),
            reply_to_message_id.clone(),
        )
    });
    if let Some(progress_relay) = progress_relay.as_ref() {
        if let Err(error) = progress_relay.mark_received().await {
            warn!("failed to publish initial progress update: {error}");
        }
    }
    let channel_identity_prompt = build_channel_identity_prompt(&runtime, &message).await;
    let event_sink = progress_relay
        .as_ref()
        .map(|relay| Arc::new(relay.clone()) as Arc<dyn LoopEventSink>);
    let mut gateway_message = build_gateway_channel_message(message)?;
    gateway_message.channel_identity_prompt = channel_identity_prompt;

    // Create a ChannelInteractionHandle so ask_user_question works in Feishu.
    let interaction_handle: Option<Arc<dyn agent_contracts::InteractionHandle>> =
        Some(Arc::new(ChannelInteractionHandle::new(
            state.interaction_timeout_secs,
            session_id,
            conversation_id.clone(),
            reply_to_message_id.clone(),
            state.pending_interactions.clone(),
            adapter.clone(),
        )));

    let turn_response = match state
        .gateway_service
        .handle_channel_message_with_interaction(gateway_message, event_sink, interaction_handle)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            if let Some(progress_relay) = progress_relay.as_ref() {
                if let Err(progress_error) = progress_relay.mark_failed(&error.to_string()).await {
                    warn!("failed to publish gateway failure progress update: {progress_error}");
                }
            }
            return Err(error.into());
        }
    };

    if let Err(error) = adapter
        .send_text(
            &conversation_id,
            &turn_response.visible_reply,
            reply_to_message_id.as_deref(),
        )
        .await
    {
        if let Some(progress_relay) = progress_relay.as_ref() {
            if let Err(progress_error) = progress_relay.mark_failed(&error.to_string()).await {
                warn!("failed to publish delivery failure progress update: {progress_error}");
            }
        }
        return Err(error.into());
    }

    if let Some(progress_relay) = progress_relay.as_ref() {
        if let Err(error) = progress_relay.mark_delivered().await {
            warn!("failed to publish delivered progress update: {error}");
        }
    }

    Ok(())
}

async fn build_channel_identity_prompt(
    runtime: &ChannelRuntime,
    message: &crate::channels::ChannelMessage,
) -> Option<String> {
    let mut participants = Vec::new();
    let mut seen_ids = HashSet::new();

    push_participant(
        &mut participants,
        &mut seen_ids,
        message.sender_id.clone(),
        None,
    );

    for mention in &message.mentions {
        push_participant(
            &mut participants,
            &mut seen_ids,
            mention.id.clone(),
            mention.display_name.clone(),
        );
    }

    if runtime.capabilities.supports_member_listing {
        match runtime.adapter.list_members(&message.conversation_id).await {
            Ok(members) => {
                for member in members {
                    push_participant(
                        &mut participants,
                        &mut seen_ids,
                        member.id,
                        member.display_name,
                    );
                }
            }
            Err(error) => {
                warn!(
                    "failed to load channel member directory: instance={} channel={} conversation={} error={}",
                    runtime.instance_id,
                    runtime.channel_id,
                    message.conversation_id,
                    error
                );
            }
        }
    }

    if participants.is_empty() {
        None
    } else {
        Some(render_participant_directory(&participants))
    }
}

fn push_participant(
    participants: &mut Vec<GatewayChannelMention>,
    seen_ids: &mut HashSet<String>,
    id: String,
    display_name: Option<String>,
) {
    let normalized_id = id.trim();
    if normalized_id.is_empty() {
        return;
    }

    if let Some(existing) = participants
        .iter_mut()
        .find(|participant| participant.id == normalized_id)
    {
        if existing
            .display_name
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            existing.display_name = normalize_display_name(display_name);
        }
        return;
    }

    if seen_ids.insert(normalized_id.to_string()) {
        participants.push(GatewayChannelMention {
            id: normalized_id.to_string(),
            display_name: normalize_display_name(display_name),
        });
    }
}

fn normalize_display_name(display_name: Option<String>) -> Option<String> {
    display_name.and_then(|display_name| {
        let trimmed = display_name.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn render_participant_directory(participants: &[GatewayChannelMention]) -> String {
    let mut rendered = String::from("<participant_directory>");
    for participant in participants {
        let label = participant
            .display_name
            .as_deref()
            .filter(|display_name| !display_name.trim().is_empty())
            .unwrap_or(participant.id.as_str());
        rendered.push_str("\n<person uid=\"");
        rendered.push_str(&escape_xml(participant.id.as_str()));
        rendered.push_str("\">");
        rendered.push_str(&escape_xml(label));
        rendered.push_str("</person>");
    }
    rendered.push_str("\n</participant_directory>");
    rendered
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn validate_test_chat_request(payload: TestChatRequest) -> Result<TestChatTurnRequest, String> {
    let channel = payload
        .channel
        .or(payload.channel_instance_id.clone())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "channel or channel_instance_id is required".to_string())?;
    let sender_id = payload
        .sender_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "sender_id is required".to_string())?;
    let conversation_id = payload
        .conversation_id
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "conversation_id is required".to_string())?;
    let text = payload.text.trim().to_string();
    if text.is_empty() {
        return Err("text must not be empty".to_string());
    }

    Ok(TestChatTurnRequest {
        text,
        channel,
        channel_instance_id: payload.channel_instance_id,
        sender_id,
        agent: payload
            .agent
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
        conversation_id,
        message_id: payload.message_id,
        reply_to_message_id: payload.reply_to_message_id,
        root_message_id: payload.root_message_id,
        mentions: payload.mentions,
    })
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
