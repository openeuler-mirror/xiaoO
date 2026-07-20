use crate::daemon_config::ResolvedMcpServerConfig;
use crate::httpserver::rate_limit::RateLimitConfig;
use agent_contracts::LoopEventSink;
use agent_types::common::ids::AgentId;
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::Response,
    Router,
};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        Meta, ProgressNotificationParam, ProgressToken, ProtocolVersion, ServerCapabilities,
        ServerInfo,
    },
    tool, tool_handler, tool_router, Json, Peer, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use tokio_util::sync::CancellationToken;
use xiaoo_shared::gateway::{
    AppTurnRequest, AppTurnResult, GatewayEntryContext, GatewayEntryKind, SessionControlPlane,
    SessionLifecycleStatus, SessionRecord, SessionService, SessionStore,
};

const CHATBOT_INSTANCE_ID: &str = "chatbot";
const AGENT_INSTANCE_ID: &str = "agent";

#[derive(Clone)]
struct McpRuntimeState {
    session_service: Arc<dyn SessionService>,
    session_store: Arc<dyn SessionStore>,
    chatbot_workspace: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum McpEndpoint {
    Chatbot,
    Agent,
}

impl McpEndpoint {
    fn instance_id(self) -> &'static str {
        match self {
            Self::Chatbot => CHATBOT_INSTANCE_ID,
            Self::Agent => AGENT_INSTANCE_ID,
        }
    }

    fn session_prefix(self) -> &'static str {
        match self {
            Self::Chatbot => "mcp_chat_",
            Self::Agent => "mcp_agent_",
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
struct ChatInput {
    /// The user message to answer.
    message: String,
    /// xiaoO application session ID returned by a previous call.
    #[serde(default)]
    session_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AgentInput {
    /// The user message or task for the xiaoO agent.
    message: String,
    /// xiaoO application session ID returned by a previous call.
    #[serde(default)]
    session_id: Option<String>,
    /// Absolute, existing, readable directory. Required for a new session.
    #[serde(default)]
    workspace: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct McpUsage {
    #[schemars(schema_with = "nonnegative_integer_schema")]
    prompt_tokens: u64,
    #[schemars(schema_with = "nonnegative_integer_schema")]
    completion_tokens: u64,
    #[schemars(schema_with = "nonnegative_integer_schema")]
    total_tokens: u64,
    #[schemars(schema_with = "nonnegative_integer_schema")]
    estimated_input_tokens: u64,
}

fn nonnegative_integer_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "integer",
        "minimum": 0
    })
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct McpTurnOutput {
    session_id: String,
    created: bool,
    reply: String,
    outcome: String,
    usage: McpUsage,
}

#[derive(Clone)]
struct ChatbotMcpServer {
    state: McpRuntimeState,
    tool_router: ToolRouter<Self>,
}

impl ChatbotMcpServer {
    fn new(state: McpRuntimeState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl ChatbotMcpServer {
    #[tool(
        name = "chat",
        description = "Chat with a minimal xiaoO assistant that can only search the web and read files in its fixed empty workspace"
    )]
    async fn chat(
        &self,
        Parameters(input): Parameters<ChatInput>,
        meta: Meta,
        peer: Peer<RoleServer>,
        cancellation_token: CancellationToken,
    ) -> Result<Json<McpTurnOutput>, String> {
        let workspace = self.state.chatbot_workspace.clone();
        run_mcp_turn(
            &self.state,
            McpEndpoint::Chatbot,
            input.message,
            input.session_id,
            Some(workspace),
            meta,
            peer,
            cancellation_token,
        )
        .await
        .map(Json)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for ChatbotMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .with_instructions(
                "This endpoint exposes only the chat tool. Reuse its returned session_id to continue a conversation.",
            )
    }
}

#[derive(Clone)]
struct AgentMcpServer {
    state: McpRuntimeState,
    tool_router: ToolRouter<Self>,
}

impl AgentMcpServer {
    fn new(state: McpRuntimeState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl AgentMcpServer {
    #[tool(
        name = "agent",
        description = "Run the full local xiaoO Core agent in a caller-bound workspace"
    )]
    async fn agent(
        &self,
        Parameters(input): Parameters<AgentInput>,
        meta: Meta,
        peer: Peer<RoleServer>,
        cancellation_token: CancellationToken,
    ) -> Result<Json<McpTurnOutput>, String> {
        let workspace = input.workspace.as_deref().map(PathBuf::from);
        run_mcp_turn(
            &self.state,
            McpEndpoint::Agent,
            input.message,
            input.session_id,
            workspace,
            meta,
            peer,
            cancellation_token,
        )
        .await
        .map(Json)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AgentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .with_instructions(
                "This endpoint exposes only the agent tool. A new session requires an absolute workspace; reuse session_id to continue.",
            )
    }
}

pub fn create_mcp_router(
    config: ResolvedMcpServerConfig,
    session_service: Arc<dyn SessionService>,
    session_control_plane: Arc<dyn SessionControlPlane>,
    session_store: Arc<dyn SessionStore>,
    rate_limit: Option<RateLimitConfig>,
) -> Router {
    spawn_idle_reaper(
        session_store.clone(),
        session_control_plane,
        config.idle_timeout_secs,
        config.reaper_interval_secs,
    );
    let state = McpRuntimeState {
        session_service,
        session_store,
        chatbot_workspace: config.chatbot_workspace,
    };

    let chatbot_state = state.clone();
    let chatbot_service: StreamableHttpService<ChatbotMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(ChatbotMcpServer::new(chatbot_state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .disable_allowed_hosts()
                .with_sse_keep_alive(None),
        );
    let agent_state = state;
    let agent_service: StreamableHttpService<AgentMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(AgentMcpServer::new(agent_state.clone())),
            Default::default(),
            StreamableHttpServerConfig::default()
                .disable_allowed_hosts()
                .with_sse_keep_alive(None),
        );

    let allowed_origins: Arc<HashSet<String>> =
        Arc::new(config.allowed_origins.into_iter().collect());
    let chatbot_auth = McpEndpointAuth {
        bearer_token: Arc::from(config.chatbot_token),
        allowed_origins: allowed_origins.clone(),
    };
    let agent_auth = McpEndpointAuth {
        bearer_token: Arc::from(config.agent_token),
        allowed_origins,
    };

    let router = Router::new()
        .merge(
            Router::new()
                .nest_service("/mcp/chatbot", chatbot_service)
                .layer(middleware::from_fn_with_state(chatbot_auth, authorize_mcp)),
        )
        .merge(
            Router::new()
                .nest_service("/mcp/agent", agent_service)
                .layer(middleware::from_fn_with_state(agent_auth, authorize_mcp)),
        );
    match rate_limit.and_then(|config| config.governor_layer()) {
        Some(layer) => router.layer(layer),
        None => router,
    }
}

fn spawn_idle_reaper(
    session_store: Arc<dyn SessionStore>,
    session_control_plane: Arc<dyn SessionControlPlane>,
    idle_timeout_secs: u64,
    reaper_interval_secs: u64,
) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(reaper_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let now_ms = current_time_ms();
            let idle_before_ms = now_ms.saturating_sub(idle_timeout_secs.saturating_mul(1_000));
            for record in session_store.list_all().await {
                if record.entry.kind != Some(GatewayEntryKind::Mcp)
                    || record.status != SessionLifecycleStatus::Idle
                    || record.updated_at_ms > idle_before_ms
                {
                    continue;
                }
                match session_control_plane
                    .hibernate_idle_session(&record.session_id, idle_before_ms)
                    .await
                {
                    Ok(Some(_)) => tracing::info!(
                        session_id = %record.session_id,
                        "hibernated idle MCP runtime; application session record retained"
                    ),
                    Ok(None) => {}
                    Err(error) => tracing::warn!(
                        session_id = %record.session_id,
                        error = %error,
                        "failed to hibernate idle MCP runtime"
                    ),
                }
            }
        }
    });
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

async fn run_mcp_turn(
    state: &McpRuntimeState,
    endpoint: McpEndpoint,
    message: String,
    supplied_session_id: Option<String>,
    supplied_workspace: Option<PathBuf>,
    meta: Meta,
    peer: Peer<RoleServer>,
    cancellation_token: CancellationToken,
) -> Result<McpTurnOutput, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("message must not be empty".to_string());
    }

    let (session_id, created, workspace) = match normalize_session_id(supplied_session_id)? {
        Some(session_id) => {
            let record = state
                .session_store
                .load(&session_id)
                .await
                .ok_or_else(|| format!("unknown session_id `{session_id}`"))?;
            let workspace = validate_existing_session(
                endpoint,
                &record,
                supplied_workspace.as_deref(),
                &state.chatbot_workspace,
            )?;
            (session_id, false, workspace)
        }
        None => {
            let workspace = validate_new_workspace(
                endpoint,
                supplied_workspace.as_deref(),
                &state.chatbot_workspace,
            )?;
            (
                format!("{}{}", endpoint.session_prefix(), uuid::Uuid::new_v4()),
                true,
                workspace,
            )
        }
    };

    let progress = meta
        .get_progress_token()
        .map(|token| Arc::new(McpProgressSink::new(peer, token)));
    let request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext {
            kind: Some(GatewayEntryKind::Mcp),
            instance_id: Some(endpoint.instance_id().to_string()),
            runtime_profile_id: None,
            build_tags: Vec::new(),
        },
        channel: None,
        message_id: None,
        conversation_id: session_id.clone(),
        sender_id: "mcp-user".to_string(),
        text: message.to_string(),
        channel_instance_id: None,
        channel_identity_prompt: None,
        reply_to_message_id: None,
        root_message_id: None,
        mentions: Vec::new(),
        reasoning_effort: Default::default(),
        llm: None,
        workspace: Some(workspace),
        skills: None,
        command_context: None,
        chain_depth: 0,
        // MCP server is a daemon-internal caller. It calls
        // `session_service.run_turn()` directly (not via the HTTP router's
        // `require_lease_holder`), and the SessionActor's pop-time check
        // allows anonymous (`None`) callers through, so MCP-initiated turns
        // run even when `XIAOO_ENFORCE_LEASE=on`. If MCP turns ever need to
        // be gated by single-writer enforcement, assign a `daemon:mcp`
        // principal here so the pop-time check bypasses explicitly (matching
        // cron / hook / channel ingress).
        client_id: None,
    };

    let event_sink = progress.map(|sink| sink as Arc<dyn LoopEventSink>);
    let result = state
        .session_service
        .run_turn_with_interaction(request, event_sink, None, None, Some(cancellation_token))
        .await
        .map_err(|error| error.to_string())?;

    Ok(turn_output(session_id, created, result))
}

fn turn_output(session_id: String, created: bool, result: AppTurnResult) -> McpTurnOutput {
    McpTurnOutput {
        session_id,
        created,
        reply: result.visible_reply,
        outcome: result.outcome.as_tag().to_string(),
        usage: McpUsage {
            prompt_tokens: result.prompt_tokens,
            completion_tokens: result.completion_tokens,
            total_tokens: result.total_tokens,
            estimated_input_tokens: result.estimated_input_tokens,
        },
    }
}

fn normalize_session_id(session_id: Option<String>) -> Result<Option<String>, String> {
    match session_id {
        None => Ok(None),
        Some(value) => {
            let value = value.trim();
            if value.is_empty() {
                Err("session_id must not be empty when provided".to_string())
            } else {
                Ok(Some(value.to_string()))
            }
        }
    }
}

fn validate_new_workspace(
    endpoint: McpEndpoint,
    supplied: Option<&Path>,
    chatbot_workspace: &Path,
) -> Result<PathBuf, String> {
    match endpoint {
        McpEndpoint::Chatbot => Ok(chatbot_workspace.to_path_buf()),
        McpEndpoint::Agent => {
            let workspace = supplied.ok_or_else(|| {
                "workspace is required when creating an agent session".to_string()
            })?;
            canonicalize_agent_workspace(workspace)
        }
    }
}

fn validate_existing_session(
    endpoint: McpEndpoint,
    record: &SessionRecord,
    supplied_workspace: Option<&Path>,
    chatbot_workspace: &Path,
) -> Result<PathBuf, String> {
    if record.entry.kind != Some(GatewayEntryKind::Mcp)
        || record.entry.instance_id.as_deref() != Some(endpoint.instance_id())
    {
        return Err(format!(
            "session_id `{}` belongs to a different endpoint",
            record.session_id
        ));
    }
    if record.status == SessionLifecycleStatus::Closed {
        return Err(format!("session_id `{}` is closed", record.session_id));
    }

    let bound = record
        .runtime
        .workspace_root
        .canonicalize()
        .map_err(|error| {
            format!(
                "bound workspace {} is unavailable: {error}",
                record.runtime.workspace_root.display()
            )
        })?;
    match endpoint {
        McpEndpoint::Chatbot => {
            if bound != chatbot_workspace {
                return Err("chatbot session workspace binding is invalid".to_string());
            }
        }
        McpEndpoint::Agent => {
            if let Some(supplied) = supplied_workspace {
                let supplied = canonicalize_agent_workspace(supplied)?;
                if supplied != bound {
                    return Err(format!(
                        "workspace conflicts with session binding {}",
                        bound.display()
                    ));
                }
            }
        }
    }
    Ok(bound)
}

fn canonicalize_agent_workspace(workspace: &Path) -> Result<PathBuf, String> {
    if !workspace.is_absolute() {
        return Err(format!(
            "workspace must be an absolute path: {}",
            workspace.display()
        ));
    }
    let canonical = workspace
        .canonicalize()
        .map_err(|error| format!("workspace {} is invalid: {error}", workspace.display()))?;
    if !canonical.is_dir() {
        return Err(format!(
            "workspace is not a directory: {}",
            canonical.display()
        ));
    }
    std::fs::read_dir(&canonical)
        .map_err(|error| format!("workspace {} is not readable: {error}", canonical.display()))?;
    Ok(canonical)
}

struct McpProgressSink {
    peer: Peer<RoleServer>,
    token: ProgressToken,
    progress: AtomicU64,
    last_snapshot_len: Mutex<HashMap<String, usize>>,
}

impl McpProgressSink {
    fn new(peer: Peer<RoleServer>, token: ProgressToken) -> Self {
        Self {
            peer,
            token,
            progress: AtomicU64::new(0),
            last_snapshot_len: Mutex::new(HashMap::new()),
        }
    }

    fn notify(&self, message: String) {
        let progress = self.progress.fetch_add(1, Ordering::SeqCst) + 1;
        let peer = self.peer.clone();
        let token = self.token.clone();
        tokio::spawn(async move {
            let _ = peer
                .notify_progress(
                    ProgressNotificationParam::new(token, progress as f64).with_message(message),
                )
                .await;
        });
    }
}

impl LoopEventSink for McpProgressSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        if let Ok(mut lengths) = self.last_snapshot_len.lock() {
            lengths.insert(agent_id.0.clone(), 0);
        }
        self.notify(format!("turn_started:{turn}"));
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        let delta = {
            let Ok(mut lengths) = self.last_snapshot_len.lock() else {
                return;
            };
            let previous = *lengths.get(&agent_id.0).unwrap_or(&0);
            lengths.insert(agent_id.0.clone(), text.len());
            if previous >= text.len() || !text.is_char_boundary(previous) {
                return;
            }
            text[previous..].to_string()
        };
        self.notify(format!("text_delta:{delta}"));
    }

    fn on_assistant_reasoning(&self, _agent_id: &AgentId, _text: &str) {}

    fn on_tool_result(&self, _agent_id: &AgentId, event: &ToolResultEvent) {
        let status = if event.is_error {
            "failed"
        } else {
            "succeeded"
        };
        self.notify(format!("tool:{}:{status}", event.tool_name));
    }

    fn on_loop_end(&self, _agent_id: &AgentId, _summary: &LoopEndSummary) {}
}

#[derive(Clone)]
struct McpEndpointAuth {
    bearer_token: Arc<str>,
    allowed_origins: Arc<HashSet<String>>,
}

async fn authorize_mcp(
    State(auth): State<McpEndpointAuth>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let origin = origin.to_str().map_err(|_| StatusCode::FORBIDDEN)?;
        if !auth.allowed_origins.contains(origin) {
            return Err(StatusCode::FORBIDDEN);
        }
    }

    let authorized = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .map(|token| token == auth.bearer_token.as_ref())
        .unwrap_or(false);
    if !authorized {
        return Err(StatusCode::UNAUTHORIZED);
    }
    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_types::context::{FeatureFlags, TokenBudgetConfig};
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::Request as HttpRequest;
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use tower::ServiceExt;
    use xiaoo_shared::gateway::{
        InMemorySessionStore, SessionControlPlane, SessionRuntimeSnapshot, SessionServiceError,
    };

    struct MockSessions;

    #[async_trait]
    impl SessionService for MockSessions {
        async fn run_turn(
            &self,
            _request: AppTurnRequest,
        ) -> Result<AppTurnResult, SessionServiceError> {
            Ok(AppTurnResult {
                raw_reply: "mock reply".to_string(),
                visible_reply: "mock reply".to_string(),
                messages: Vec::new(),
                prompt_tokens: 3,
                completion_tokens: 2,
                total_tokens: 5,
                estimated_input_tokens: 3,
                outcome: xiaoo_shared::gateway::TurnOutcome::Complete,
                hook_actions: Vec::new(),
            })
        }
    }

    #[async_trait]
    impl SessionControlPlane for MockSessions {}

    #[test]
    fn agent_workspace_must_be_absolute_and_existing() {
        assert!(canonicalize_agent_workspace(Path::new("relative")).is_err());
        assert!(
            canonicalize_agent_workspace(Path::new("/definitely/not/a/xiaoo/workspace")).is_err()
        );

        let workspace = TempDir::new().expect("temp workspace");
        assert_eq!(
            canonicalize_agent_workspace(workspace.path()).expect("valid workspace"),
            workspace
                .path()
                .canonicalize()
                .expect("canonical workspace")
        );
    }

    #[test]
    fn empty_supplied_session_id_is_rejected() {
        assert!(normalize_session_id(Some("  ".to_string())).is_err());
        assert_eq!(normalize_session_id(None).expect("new session"), None);
    }

    #[test]
    fn output_schema_uses_portable_nonnegative_integers_for_usage() {
        let schema = serde_json::to_value(schemars::schema_for!(McpTurnOutput))
            .expect("output schema should serialize");
        let schema_text = schema.to_string();
        assert!(!schema_text.contains("uint64"), "{schema_text}");

        for field in [
            "prompt_tokens",
            "completion_tokens",
            "total_tokens",
            "estimated_input_tokens",
        ] {
            let field_schema = schema
                .pointer(&format!("/$defs/McpUsage/properties/{field}"))
                .unwrap_or_else(|| panic!("missing usage schema for {field}: {schema_text}"));
            assert_eq!(
                field_schema.get("type"),
                Some(&serde_json::json!("integer"))
            );
            assert_eq!(field_schema.get("minimum"), Some(&serde_json::json!(0)));
            assert!(field_schema.get("format").is_none(), "{field_schema}");
        }
    }

    fn session_record(
        session_id: &str,
        endpoint: McpEndpoint,
        workspace: PathBuf,
    ) -> SessionRecord {
        SessionRecord {
            session_id: session_id.to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "sender".to_string(),
            entry: GatewayEntryContext {
                kind: Some(GatewayEntryKind::Mcp),
                instance_id: Some(endpoint.instance_id().to_string()),
                runtime_profile_id: None,
                build_tags: Vec::new(),
            },
            channel: None,
            channel_instance_id: None,
            status: SessionLifecycleStatus::Idle,
            runtime: SessionRuntimeSnapshot {
                agent_id: AgentId("core".to_string()),
                model: "test-model".to_string(),
                llm: None,
                system_prompt: "test".to_string(),
                feature_flags: FeatureFlags::default(),
                token_budget: TokenBudgetConfig {
                    total_budget: 16_384,
                    reserved_for_output: 2_048,
                    reserved_for_system: 2_048,
                    hard_limit_ratio: 1.0,
                },
                workspace_root: workspace,
                max_turns: None,
                tool_manifest: None,
                subagent_roles: BTreeMap::new(),
                bootstrap_binding: None,
            },
            backend_instance: None,
            paused_backend_checkpoint: None,
            loop_state: None,
            memory_snapshot: None,
            agents: BTreeMap::new(),
            subagent_state: Default::default(),
            last_error: None,
            parent_runtime_id: None,
            forked_from_checkpoint_id: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn existing_session_rejects_cross_endpoint_and_workspace_conflict() {
        let chatbot_workspace = TempDir::new().expect("chatbot workspace");
        let agent_workspace = TempDir::new().expect("agent workspace");
        let other_workspace = TempDir::new().expect("other workspace");
        let record = session_record(
            "mcp_agent_test",
            McpEndpoint::Agent,
            agent_workspace.path().canonicalize().expect("agent path"),
        );

        let cross_endpoint = validate_existing_session(
            McpEndpoint::Chatbot,
            &record,
            None,
            chatbot_workspace.path(),
        )
        .expect_err("cross endpoint session must be rejected");
        assert!(cross_endpoint.contains("different endpoint"));

        let conflict = validate_existing_session(
            McpEndpoint::Agent,
            &record,
            Some(other_workspace.path()),
            chatbot_workspace.path(),
        )
        .expect_err("workspace conflict must be rejected");
        assert!(conflict.contains("workspace conflicts"));

        let inherited =
            validate_existing_session(McpEndpoint::Agent, &record, None, chatbot_workspace.path())
                .expect("omitted workspace should inherit the session binding");
        assert_eq!(
            inherited,
            agent_workspace.path().canonicalize().expect("agent path")
        );
    }

    fn test_router(workspace: &Path) -> Router {
        let sessions = Arc::new(MockSessions);
        create_mcp_router(
            ResolvedMcpServerConfig {
                idle_timeout_secs: 3_600,
                reaper_interval_secs: 3_600,
                allowed_origins: Vec::new(),
                chatbot_token: "chat-token".to_string(),
                chatbot_workspace: workspace.to_path_buf(),
                agent_token: "agent-token".to_string(),
            },
            sessions.clone(),
            sessions,
            Arc::new(InMemorySessionStore::default()),
            None,
        )
    }

    fn mcp_post(path: &str, token: Option<&str>, body: serde_json::Value) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder()
            .method("POST")
            .uri(path)
            .header(header::HOST, "daemon.example.com")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream");
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::from(body.to_string())).expect("request")
    }

    #[tokio::test]
    async fn mcp_transport_requires_endpoint_auth_and_lists_only_chat() {
        let workspace = TempDir::new().expect("workspace");
        let router = test_router(workspace.path());

        let unauthorized = router
            .clone()
            .oneshot(mcp_post(
                "/mcp/chatbot",
                None,
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "test", "version": "1"}
                    }
                }),
            ))
            .await
            .expect("response");
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let wrong_endpoint_token = router
            .clone()
            .oneshot(mcp_post(
                "/mcp/chatbot",
                Some("agent-token"),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "test", "version": "1"}
                    }
                }),
            ))
            .await
            .expect("response");
        assert_eq!(wrong_endpoint_token.status(), StatusCode::UNAUTHORIZED);

        let initialized = router
            .clone()
            .oneshot(mcp_post(
                "/mcp/chatbot",
                Some("chat-token"),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "test", "version": "1"}
                    }
                }),
            ))
            .await
            .expect("initialize response");
        if initialized.status() != StatusCode::OK {
            let status = initialized.status();
            let body = to_bytes(initialized.into_body(), 1024 * 1024)
                .await
                .expect("initialize error body");
            panic!(
                "initialize returned {status}: {}",
                String::from_utf8_lossy(&body)
            );
        }
        let mcp_session_id = initialized
            .headers()
            .get("mcp-session-id")
            .expect("MCP session header")
            .to_str()
            .expect("session header UTF-8")
            .to_string();

        let mut list_request = mcp_post(
            "/mcp/chatbot",
            Some("chat-token"),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }),
        );
        list_request.headers_mut().insert(
            "mcp-session-id",
            mcp_session_id.parse().expect("session header"),
        );
        list_request.headers_mut().insert(
            "mcp-protocol-version",
            "2025-11-25".parse().expect("protocol header"),
        );
        let listed = router
            .clone()
            .oneshot(list_request)
            .await
            .expect("tools/list response");
        assert_eq!(listed.status(), StatusCode::OK);
        let body = to_bytes(listed.into_body(), 1024 * 1024)
            .await
            .expect("response body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
        assert!(body.contains("\"name\":\"chat\""), "{body}");
        assert!(!body.contains("\"name\":\"agent\""), "{body}");

        let mut call_request = mcp_post(
            "/mcp/chatbot",
            Some("chat-token"),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": {
                    "name": "chat",
                    "arguments": {"message": "hello"}
                }
            }),
        );
        call_request.headers_mut().insert(
            "mcp-session-id",
            mcp_session_id.parse().expect("session header"),
        );
        call_request.headers_mut().insert(
            "mcp-protocol-version",
            "2025-11-25".parse().expect("protocol header"),
        );
        let called = router
            .oneshot(call_request)
            .await
            .expect("tools/call response");
        assert_eq!(called.status(), StatusCode::OK);
        let body = to_bytes(called.into_body(), 1024 * 1024)
            .await
            .expect("call body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
        assert!(body.contains("structuredContent"), "{body}");
        assert!(body.contains("mcp_chat_"), "{body}");
        assert!(body.contains("mock reply"), "{body}");
    }

    #[tokio::test]
    async fn browser_origin_is_denied_by_default() {
        let workspace = TempDir::new().expect("workspace");
        let router = test_router(workspace.path());
        let mut request = mcp_post(
            "/mcp/chatbot",
            Some("chat-token"),
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "1"}
                }
            }),
        );
        request
            .headers_mut()
            .insert(header::ORIGIN, "https://example.com".parse().unwrap());
        let response = router.oneshot(request).await.expect("response");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn auth_covers_stream_get_and_session_delete() {
        let workspace = TempDir::new().expect("workspace");
        let router = test_router(workspace.path());
        for method in ["GET", "DELETE"] {
            let request = HttpRequest::builder()
                .method(method)
                .uri("/mcp/chatbot")
                .header(header::HOST, "localhost")
                .body(Body::empty())
                .expect("request");
            let response = router.clone().oneshot(request).await.expect("response");
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{method}");
        }
    }

    #[tokio::test]
    async fn agent_endpoint_lists_only_agent_and_requires_workspace_for_new_call() {
        let workspace = TempDir::new().expect("workspace");
        let router = test_router(workspace.path());
        let initialized = router
            .clone()
            .oneshot(mcp_post(
                "/mcp/agent",
                Some("agent-token"),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "protocolVersion": "2025-11-25",
                        "capabilities": {},
                        "clientInfo": {"name": "test", "version": "1"}
                    }
                }),
            ))
            .await
            .expect("initialize response");
        assert_eq!(initialized.status(), StatusCode::OK);
        let mcp_session_id = initialized
            .headers()
            .get("mcp-session-id")
            .expect("MCP session header")
            .to_str()
            .expect("session header UTF-8")
            .to_string();

        let request = |id, method: &str, params: serde_json::Value| {
            let mut request = mcp_post(
                "/mcp/agent",
                Some("agent-token"),
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": method,
                    "params": params
                }),
            );
            request.headers_mut().insert(
                "mcp-session-id",
                mcp_session_id.parse().expect("session header"),
            );
            request.headers_mut().insert(
                "mcp-protocol-version",
                "2025-11-25".parse().expect("protocol header"),
            );
            request
        };
        let listed = router
            .clone()
            .oneshot(request(2, "tools/list", serde_json::json!({})))
            .await
            .expect("tools/list response");
        let body = to_bytes(listed.into_body(), 1024 * 1024)
            .await
            .expect("list body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
        assert!(body.contains("\"name\":\"agent\""), "{body}");
        assert!(!body.contains("\"name\":\"chat\""), "{body}");

        let called = router
            .oneshot(request(
                3,
                "tools/call",
                serde_json::json!({
                    "name": "agent",
                    "arguments": {"message": "hello"}
                }),
            ))
            .await
            .expect("tools/call response");
        let body = to_bytes(called.into_body(), 1024 * 1024)
            .await
            .expect("call body");
        let body = String::from_utf8(body.to_vec()).expect("UTF-8 body");
        assert!(body.contains("workspace is required"), "{body}");
        assert!(body.contains("\"isError\":true"), "{body}");
    }
}
