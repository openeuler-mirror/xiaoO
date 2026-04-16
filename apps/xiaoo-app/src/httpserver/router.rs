use crate::channels::feishu::{FeishuAdapter, FeishuConfig};
use crate::channels::{
    feishu_capabilities, feishu_meta, AdapterResponse, ChannelAdapter, ChannelError,
    ChannelOutboundAttachment, ChannelOutboundAttachmentKind, ChannelResult, ChannelRuntime,
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
use crate::httpserver::{GatewayService, GatewayServiceError};
use agent_contracts::{ChannelFileSender, LoopEventSink};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
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
            interaction_timeout_secs: 600,
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
            interaction_timeout_secs: 600,
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
        .route("/api/v1/chat", post(handle_test_chat))
        .route("/api/v1/channels/feishu/events", post(handle_feishu_events))
        .with_state(Arc::new(state))
}

async fn health_check() -> Json<GatewayHealthResponse> {
    Json(GatewayHealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

async fn handle_test_chat(
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
                    if let Err(error) = runtime.adapter.acknowledge_message(&message.message_id).await
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
        .handle_channel_message_with_interaction(
            gateway_message,
            event_sink,
            interaction_handle,
            Some(Arc::new(AdapterFileSender {
                adapter: adapter.clone(),
                conversation_id: conversation_id.clone(),
                reply_to_message_id: reply_to_message_id.clone(),
            })),
        )
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

// ---------------------------------------------------------------------------
// AdapterFileSender — wraps a ChannelAdapter to implement ChannelFileSender
// ---------------------------------------------------------------------------

struct AdapterFileSender {
    adapter: Arc<dyn ChannelAdapter>,
    conversation_id: String,
    reply_to_message_id: Option<String>,
}

#[async_trait::async_trait]
impl ChannelFileSender for AdapterFileSender {
    async fn send_file(
        &self,
        file_path: &str,
        label: Option<&str>,
    ) -> Result<Option<String>, String> {
        let attachment = ChannelOutboundAttachment {
            kind: ChannelOutboundAttachmentKind::File,
            path: file_path.to_string(),
            label: label.map(ToString::to_string),
        };
        self.adapter
            .send_attachment(
                &self.conversation_id,
                &attachment,
                self.reply_to_message_id.as_deref(),
            )
            .await
            .map_err(|e| e.to_string())
    }

    fn conversation_id(&self) -> &str {
        &self.conversation_id
    }
}

#[cfg(test)]
mod tests {
    use super::{
        handle_feishu_events, validate_test_chat_request, GatewayAppState, TestChatMention,
        TestChatRequest,
    };
    use crate::channels::{
        AdapterResponse, ChannelAdapter, ChannelCapabilities, ChannelMember, ChannelMention,
        ChannelMessage, ChannelMeta, ChannelResult, ChannelRuntime, ChannelTextFormat,
    };
    use crate::gateway::{AppTurnRequest, AppTurnResult, SessionService, SessionServiceError};
    use agent_contracts::LoopEventSink;
    use async_trait::async_trait;
    use axum::{
        body::Bytes,
        extract::State,
        http::{HeaderMap, StatusCode},
    };
    use std::sync::{Arc, Mutex};
    use tokio::time::{sleep, timeout, Duration};

    #[test]
    fn rejects_missing_identity_fields() {
        let error = validate_test_chat_request(TestChatRequest {
            text: "hello".to_string(),
            channel: None,
            channel_instance_id: None,
            sender_id: None,
            conversation_id: None,
            message_id: None,
            reply_to_message_id: None,
            root_message_id: None,
            mentions: Vec::new(),
        })
        .expect_err("request should fail fast");

        assert_eq!(error, "channel or channel_instance_id is required");
    }

    #[test]
    fn accepts_explicit_test_chat_request() {
        let request = validate_test_chat_request(TestChatRequest {
            text: "  hello  ".to_string(),
            channel: Some("feishu".to_string()),
            channel_instance_id: Some("ops-feishu".to_string()),
            sender_id: Some("user-1".to_string()),
            conversation_id: Some("conv-1".to_string()),
            message_id: Some("msg-1".to_string()),
            reply_to_message_id: None,
            root_message_id: None,
            mentions: vec![TestChatMention {
                id: "bot".to_string(),
                display_name: Some("XiaoO".to_string()),
            }],
        })
        .expect("request should be valid");

        assert_eq!(request.text, "hello");
        assert_eq!(request.channel, "feishu");
        assert_eq!(request.channel_instance_id.as_deref(), Some("ops-feishu"));
        assert_eq!(request.mentions.len(), 1);
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

        let response =
            handle_feishu_events(State(state), HeaderMap::new(), Bytes::from_static(b"{}")).await;

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

        let response =
            handle_feishu_events(State(state), HeaderMap::new(), Bytes::from_static(b"{}")).await;

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
            handle_feishu_events(State(state), HeaderMap::new(), Bytes::from_static(b"{}")),
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
