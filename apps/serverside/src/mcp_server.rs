use crate::daemon_config::ResolvedMcpServerConfig;
use crate::httpserver::rate_limit::RateLimitConfig;
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
        Implementation, Meta, ProgressNotificationParam, ProgressToken, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router, Json, Peer, RoleServer, ServerHandler,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use xiaoo_api::chat::AgentId;
use xiaoo_api::events::{LoopEndSummary, LoopEventSink, ToolResultEvent};
use xiaoo_shared::gateway::{
    AppTurnRequest, AppTurnResult, GatewayEntryContext, GatewayEntryKind, SessionControlPlane,
    SessionLifecycleStatus, SessionRecord, SessionService, SessionStore,
};

const CHATBOT_INSTANCE_ID: &str = "chatbot";
const AGENT_INSTANCE_ID: &str = "agent";
const AGENT_POLL_AFTER_MS: u64 = 30_000;
const COMPLETED_OPERATIONS_PER_SESSION: usize = 16;
const COMPLETED_OPERATION_RETENTION: Duration = Duration::from_secs(10 * 60);

#[derive(Clone)]
struct McpRuntimeState {
    session_service: Arc<dyn SessionService>,
    session_store: Arc<dyn SessionStore>,
    chatbot_workspace: PathBuf,
    agent_role: Option<String>,
    agent_operations: Arc<AgentOperationRegistry>,
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

#[derive(Debug, Deserialize, JsonSchema)]
struct AgentStatusInput {
    /// Operation ID returned by agent.
    operation_id: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct McpUsage {
    /// Input tokens reported for this turn.
    #[schemars(schema_with = "nonnegative_integer_schema")]
    prompt_tokens: u64,
    /// Output tokens reported for this turn.
    #[schemars(schema_with = "nonnegative_integer_schema")]
    completion_tokens: u64,
    /// Sum of reported input and output tokens.
    #[schemars(schema_with = "nonnegative_integer_schema")]
    total_tokens: u64,
    /// Locally estimated input tokens when provider usage is incomplete.
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
    /// xiaoO application session ID. Pass it to the next call to continue.
    session_id: String,
    /// True when this call created the application session.
    created: bool,
    /// User-visible assistant reply.
    reply: String,
    /// Turn outcome: complete, max_turns_reached, budget_exhausted, or cancelled.
    outcome: String,
    /// Token accounting for this turn.
    usage: McpUsage,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum AgentOperationPhase {
    Queued,
    Running,
}

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
struct AgentRunningSnapshot {
    /// Whether the operation is waiting to start or actively running.
    phase: AgentOperationPhase,
    /// Current root-agent model turn, when execution has started.
    #[schemars(schema_with = "optional_nonnegative_integer_schema")]
    current_turn: Option<u32>,
    /// Latest visible text snapshot from the current or previous root-agent turn.
    last_text: Option<String>,
    /// Unix timestamp of the most recent snapshot update.
    #[schemars(schema_with = "nonnegative_integer_schema")]
    updated_at_ms: u64,
}

fn optional_nonnegative_integer_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": ["integer", "null"],
        "minimum": 0
    })
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
struct AgentOperationOutput {
    operation_id: String,
    session_id: String,
    created: bool,
    #[serde(flatten)]
    detail: AgentOperationDetail,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
enum AgentOperationDetail {
    Running {
        #[schemars(schema_with = "nonnegative_integer_schema")]
        poll_after_ms: u64,
        snapshot: AgentRunningSnapshot,
    },
    Done {
        reply: String,
        outcome: String,
        usage: Option<McpUsage>,
        error: Option<String>,
    },
}

struct AgentOperationRegistry {
    inner: Mutex<AgentOperationRegistryInner>,
}

#[derive(Default)]
struct AgentOperationRegistryInner {
    operations: HashMap<String, AgentOperationRecord>,
    active_by_session: HashMap<String, String>,
    completed_by_session: HashMap<String, VecDeque<String>>,
}

struct AgentOperationRecord {
    operation_id: String,
    session_id: String,
    workspace: PathBuf,
    created: bool,
    state: AgentOperationState,
    cancellation_token: Option<CancellationToken>,
    completion_tx: watch::Sender<bool>,
    next_poll_at: Instant,
    completed_at: Option<Instant>,
}

enum AgentOperationState {
    Running(AgentRunningSnapshot),
    Done {
        reply: String,
        outcome: String,
        usage: Option<McpUsage>,
        error: Option<String>,
    },
}

#[derive(Clone)]
struct ActiveAgentOperation {
    operation_id: String,
    workspace: PathBuf,
}

enum AgentStatusPoll {
    Ready(AgentOperationOutput),
    Wait {
        completion_rx: watch::Receiver<bool>,
        delay: Duration,
    },
}

impl Default for AgentOperationRegistry {
    fn default() -> Self {
        Self {
            inner: Mutex::new(AgentOperationRegistryInner::default()),
        }
    }
}

impl AgentOperationRegistry {
    fn active_for_session(&self, session_id: &str) -> Option<ActiveAgentOperation> {
        let inner = self.inner.lock().ok()?;
        let operation_id = inner.active_by_session.get(session_id)?;
        let record = inner.operations.get(operation_id)?;
        Some(ActiveAgentOperation {
            operation_id: operation_id.clone(),
            workspace: record.workspace.clone(),
        })
    }

    fn register(
        &self,
        session_id: String,
        workspace: PathBuf,
        created: bool,
    ) -> Result<(String, CancellationToken, AgentOperationOutput), String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "agent operation registry is unavailable".to_string())?;
        if let Some(operation_id) = inner.active_by_session.get(&session_id) {
            return Err(format!(
                "session `{session_id}` is busy; poll agent_status with operation_id `{operation_id}`"
            ));
        }

        let operation_id = format!("mcp_op_{}", uuid::Uuid::new_v4());
        let cancellation_token = CancellationToken::new();
        let (completion_tx, _completion_rx) = watch::channel(false);
        let snapshot = AgentRunningSnapshot {
            phase: AgentOperationPhase::Queued,
            current_turn: None,
            last_text: None,
            updated_at_ms: current_time_ms(),
        };
        let output = AgentOperationOutput {
            operation_id: operation_id.clone(),
            session_id: session_id.clone(),
            created,
            detail: AgentOperationDetail::Running {
                poll_after_ms: AGENT_POLL_AFTER_MS,
                snapshot: snapshot.clone(),
            },
        };
        inner.operations.insert(
            operation_id.clone(),
            AgentOperationRecord {
                operation_id: operation_id.clone(),
                session_id: session_id.clone(),
                workspace,
                created,
                state: AgentOperationState::Running(snapshot),
                cancellation_token: Some(cancellation_token.clone()),
                completion_tx,
                next_poll_at: Instant::now() + Duration::from_millis(AGENT_POLL_AFTER_MS),
                completed_at: None,
            },
        );
        inner
            .active_by_session
            .insert(session_id, operation_id.clone());
        Ok((operation_id, cancellation_token, output))
    }

    fn poll(&self, operation_id: &str) -> Result<AgentStatusPoll, String> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| "agent operation registry is unavailable".to_string())?;
        let record = inner
            .operations
            .get_mut(operation_id)
            .ok_or_else(|| format!("unknown or expired operation_id `{operation_id}`"))?;
        if matches!(record.state, AgentOperationState::Done { .. }) {
            return Ok(AgentStatusPoll::Ready(record.output()));
        }

        let now = Instant::now();
        if now >= record.next_poll_at {
            record.next_poll_at = now + Duration::from_millis(AGENT_POLL_AFTER_MS);
            return Ok(AgentStatusPoll::Ready(record.output()));
        }
        Ok(AgentStatusPoll::Wait {
            completion_rx: record.completion_tx.subscribe(),
            delay: record.next_poll_at.saturating_duration_since(now),
        })
    }

    fn mark_running(&self, operation_id: &str, turn: u32) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(record) = inner.operations.get_mut(operation_id) else {
            return;
        };
        let AgentOperationState::Running(snapshot) = &mut record.state else {
            return;
        };
        snapshot.phase = AgentOperationPhase::Running;
        snapshot.current_turn = Some(turn);
        snapshot.updated_at_ms = current_time_ms();
    }

    fn update_last_text(&self, operation_id: &str, text: &str) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(record) = inner.operations.get_mut(operation_id) else {
            return;
        };
        let AgentOperationState::Running(snapshot) = &mut record.state else {
            return;
        };
        snapshot.last_text = Some(text.to_string());
        snapshot.updated_at_ms = current_time_ms();
    }

    fn reap_expired_completed(&self, retention: Duration) -> usize {
        let Ok(mut inner) = self.inner.lock() else {
            return 0;
        };
        let now = Instant::now();
        let expired_ids: HashSet<String> = inner
            .operations
            .iter()
            .filter_map(|(operation_id, record)| {
                record.completed_at.and_then(|completed_at| {
                    (now.saturating_duration_since(completed_at) >= retention)
                        .then(|| operation_id.clone())
                })
            })
            .collect();
        if expired_ids.is_empty() {
            return 0;
        }

        for operation_id in &expired_ids {
            inner.operations.remove(operation_id);
        }
        inner.completed_by_session.retain(|_, operation_ids| {
            operation_ids.retain(|operation_id| !expired_ids.contains(operation_id));
            !operation_ids.is_empty()
        });
        expired_ids.len()
    }

    fn complete_success(&self, operation_id: &str, result: AppTurnResult) {
        let usage = McpUsage {
            prompt_tokens: result.prompt_tokens,
            completion_tokens: result.completion_tokens,
            total_tokens: result.total_tokens,
            estimated_input_tokens: result.estimated_input_tokens,
        };
        self.complete(
            operation_id,
            result.visible_reply,
            result.outcome.as_tag().to_string(),
            Some(usage),
            None,
        );
    }

    fn complete_failure(&self, operation_id: &str, error: String) {
        let reply = self
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.operations.get(operation_id).map(last_operation_text))
            .flatten()
            .unwrap_or_default();
        self.complete(operation_id, reply, "failed".to_string(), None, Some(error));
    }

    fn complete(
        &self,
        operation_id: &str,
        reply: String,
        outcome: String,
        usage: Option<McpUsage>,
        error: Option<String>,
    ) {
        let Ok(mut inner) = self.inner.lock() else {
            return;
        };
        let Some(record) = inner.operations.get_mut(operation_id) else {
            return;
        };
        let session_id = record.session_id.clone();
        record.state = AgentOperationState::Done {
            reply,
            outcome,
            usage,
            error,
        };
        record.cancellation_token = None;
        record.completed_at = Some(Instant::now());
        let _ = record.completion_tx.send(true);
        if inner.active_by_session.get(&session_id).map(String::as_str) == Some(operation_id) {
            inner.active_by_session.remove(&session_id);
        }
        let completed = inner.completed_by_session.entry(session_id).or_default();
        completed.push_back(operation_id.to_string());
        let mut expired = Vec::new();
        while completed.len() > COMPLETED_OPERATIONS_PER_SESSION {
            if let Some(expired_id) = completed.pop_front() {
                expired.push(expired_id);
            }
        }
        for expired_id in expired {
            inner.operations.remove(&expired_id);
        }
    }
}

impl AgentOperationRecord {
    fn output(&self) -> AgentOperationOutput {
        let detail = match &self.state {
            AgentOperationState::Running(snapshot) => AgentOperationDetail::Running {
                poll_after_ms: AGENT_POLL_AFTER_MS,
                snapshot: snapshot.clone(),
            },
            AgentOperationState::Done {
                reply,
                outcome,
                usage,
                error,
            } => AgentOperationDetail::Done {
                reply: reply.clone(),
                outcome: outcome.clone(),
                usage: usage.clone(),
                error: error.clone(),
            },
        };
        AgentOperationOutput {
            operation_id: self.operation_id.clone(),
            session_id: self.session_id.clone(),
            created: self.created,
            detail,
        }
    }
}

fn last_operation_text(record: &AgentOperationRecord) -> Option<String> {
    match &record.state {
        AgentOperationState::Running(snapshot) => snapshot.last_text.clone(),
        AgentOperationState::Done { reply, .. } => Some(reply.clone()),
    }
}

async fn poll_agent_operation(
    operations: &AgentOperationRegistry,
    operation_id: &str,
) -> Result<AgentOperationOutput, String> {
    loop {
        match operations.poll(operation_id)? {
            AgentStatusPoll::Ready(output) => return Ok(output),
            AgentStatusPoll::Wait {
                mut completion_rx,
                delay,
            } => {
                tokio::select! {
                    _ = tokio::time::sleep(delay) => {}
                    changed = completion_rx.changed() => {
                        if changed.is_err() {
                            return Err(format!(
                                "operation `{operation_id}` became unavailable while waiting"
                            ));
                        }
                    }
                }
            }
        }
    }
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
        description = "Ask a web-only xiaoO chatbot. It can use web_search and webfetch, but has no file, shell, skill, plugin, planning, or subagent access. Omit session_id to start; reuse the returned session_id to continue."
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
            .with_server_info(Implementation::new(
                "xiaoo-chatbot",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Web-only chatbot endpoint. Call chat with message and no session_id to create a conversation. The result returns session_id; pass it unchanged on later calls to continue. This endpoint cannot access local files or run commands.",
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
        description = "Start a full local xiaoO Core agent operation with file, shell, skill, plugin, and subagent capabilities, then return immediately. A new session requires an absolute existing workspace. The result is running with an operation_id; poll agent_status until state is done before starting another operation in the same session."
    )]
    async fn agent(
        &self,
        Parameters(input): Parameters<AgentInput>,
    ) -> Result<Json<AgentOperationOutput>, String> {
        let workspace = input.workspace.as_deref().map(PathBuf::from);
        start_mcp_agent_operation(&self.state, input.message, input.session_id, workspace)
            .await
            .map(Json)
    }

    #[tool(
        name = "agent_status",
        description = "Poll an operation returned by agent. The server enforces the poll_after_ms interval: an early request waits until the next poll is due, or returns sooner when the operation finishes. When state is running, do not call again before poll_after_ms; last_text is only the latest root-agent turn snapshot. When state is done, use reply as the complete result."
    )]
    async fn agent_status(
        &self,
        Parameters(input): Parameters<AgentStatusInput>,
    ) -> Result<Json<AgentOperationOutput>, String> {
        let operation_id = input.operation_id.trim();
        if operation_id.is_empty() {
            return Err("operation_id must not be empty".to_string());
        }
        poll_agent_operation(&self.state.agent_operations, operation_id)
            .await
            .map(Json)
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AgentMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2025_11_25)
            .with_server_info(Implementation::new(
                "xiaoo-agent",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "Full local agent endpoint. Call agent with message plus an absolute existing workspace and no session_id to create a session. agent returns immediately with state=running and an operation_id. Wait at least poll_after_ms, then call agent_status until state=done; the server also holds early status requests until the interval elapses or the operation finishes. Never treat running.snapshot.last_text as final; only done.reply is complete. Do not call agent again for the same session until the current operation is done. Reuse the returned session_id for later operations; workspace may then be omitted, or must match the original binding. The agent runs with the daemon Unix user's effective permissions.",
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
    let agent_operations = Arc::new(AgentOperationRegistry::default());
    spawn_idle_reaper(
        session_store.clone(),
        session_control_plane,
        agent_operations.clone(),
        config.idle_timeout_secs,
        config.reaper_interval_secs,
    );
    let state = McpRuntimeState {
        session_service,
        session_store,
        chatbot_workspace: config.chatbot_workspace,
        agent_role: config.agent_role,
        agent_operations,
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
    agent_operations: Arc<AgentOperationRegistry>,
    idle_timeout_secs: u64,
    reaper_interval_secs: u64,
) {
    tokio::spawn(async move {
        let mut interval =
            tokio::time::interval(std::time::Duration::from_secs(reaper_interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            let expired_operations =
                agent_operations.reap_expired_completed(COMPLETED_OPERATION_RETENTION);
            if expired_operations > 0 {
                tracing::debug!(
                    expired_operations,
                    "removed expired completed MCP agent operations"
                );
            }
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
    let prepared = prepare_mcp_turn(
        state,
        endpoint,
        message,
        supplied_session_id,
        supplied_workspace,
    )
    .await?;
    let progress = meta
        .get_progress_token()
        .map(|token| Arc::new(McpProgressSink::new(peer, token)));
    let event_sink = progress.map(|sink| sink as Arc<dyn LoopEventSink>);
    let result = state
        .session_service
        .run_turn_with_interaction(
            prepared.request,
            event_sink,
            None,
            None,
            Some(cancellation_token),
            None,
        )
        .await
        .map_err(|error| error.to_string())?;

    Ok(turn_output(prepared.session_id, prepared.created, result))
}

struct PreparedMcpTurn {
    session_id: String,
    created: bool,
    workspace: PathBuf,
    request: AppTurnRequest,
}

async fn prepare_mcp_turn(
    state: &McpRuntimeState,
    endpoint: McpEndpoint,
    message: String,
    supplied_session_id: Option<String>,
    supplied_workspace: Option<PathBuf>,
) -> Result<PreparedMcpTurn, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("message must not be empty".to_string());
    }

    let (session_id, created, workspace, runtime_profile_id) =
        match normalize_session_id(supplied_session_id)? {
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
                let runtime_profile_id = record.entry.runtime_profile_id.clone();
                (session_id, false, workspace, runtime_profile_id)
            }
            None => {
                let workspace = validate_new_workspace(
                    endpoint,
                    supplied_workspace.as_deref(),
                    &state.chatbot_workspace,
                )?;
                let runtime_profile_id = match endpoint {
                    McpEndpoint::Chatbot => None,
                    McpEndpoint::Agent => state.agent_role.clone(),
                };
                (
                    format!("{}{}", endpoint.session_prefix(), uuid::Uuid::new_v4()),
                    true,
                    workspace,
                    runtime_profile_id,
                )
            }
        };
    let request = AppTurnRequest {
        session_id: session_id.clone(),
        entry: GatewayEntryContext {
            kind: Some(GatewayEntryKind::Mcp),
            instance_id: Some(endpoint.instance_id().to_string()),
            runtime_profile_id,
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
        reasoning_effort: None,
        llm: None,
        workspace: Some(workspace.clone()),
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

    Ok(PreparedMcpTurn {
        session_id,
        created,
        workspace,
        request,
    })
}

async fn start_mcp_agent_operation(
    state: &McpRuntimeState,
    message: String,
    supplied_session_id: Option<String>,
    supplied_workspace: Option<PathBuf>,
) -> Result<AgentOperationOutput, String> {
    let normalized_session_id = normalize_session_id(supplied_session_id.clone())?;
    if let Some(session_id) = normalized_session_id.as_deref() {
        if let Some(active) = state.agent_operations.active_for_session(session_id) {
            validate_active_agent_workspace(
                supplied_workspace.as_deref(),
                active.workspace.as_path(),
            )?;
            return Err(format!(
                "session `{session_id}` is busy; poll agent_status with operation_id `{}`",
                active.operation_id
            ));
        }
    }

    let prepared = prepare_mcp_turn(
        state,
        McpEndpoint::Agent,
        message,
        normalized_session_id,
        supplied_workspace,
    )
    .await?;
    let (operation_id, cancellation_token, initial_output) = state.agent_operations.register(
        prepared.session_id.clone(),
        prepared.workspace,
        prepared.created,
    )?;
    let progress_sink: Arc<dyn LoopEventSink> = Arc::new(AgentOperationProgressSink::new(
        state.agent_operations.clone(),
        operation_id.clone(),
    ));
    let session_service = state.session_service.clone();
    let request = prepared.request;
    let worker = tokio::spawn(async move {
        session_service
            .run_turn_with_interaction(
                request,
                Some(progress_sink),
                None,
                None,
                Some(cancellation_token),
                None,
            )
            .await
    });
    let operations = state.agent_operations.clone();
    tokio::spawn(async move {
        match worker.await {
            Ok(Ok(result)) => operations.complete_success(&operation_id, result),
            Ok(Err(error)) => {
                tracing::warn!(
                    operation_id = %operation_id,
                    error = %error,
                    "background MCP agent operation failed"
                );
                operations.complete_failure(
                    &operation_id,
                    "agent operation failed; inspect daemon logs".to_string(),
                );
            }
            Err(error) => {
                tracing::error!(
                    operation_id = %operation_id,
                    error = %error,
                    "background MCP agent operation terminated unexpectedly"
                );
                operations.complete_failure(
                    &operation_id,
                    "agent operation terminated unexpectedly; inspect daemon logs".to_string(),
                );
            }
        }
    });
    Ok(initial_output)
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

fn validate_active_agent_workspace(supplied: Option<&Path>, bound: &Path) -> Result<(), String> {
    let Some(supplied) = supplied else {
        return Ok(());
    };
    let supplied = canonicalize_agent_workspace(supplied)?;
    if supplied != bound {
        return Err(format!(
            "workspace conflicts with the session binding: expected {}, got {}",
            bound.display(),
            supplied.display()
        ));
    }
    Ok(())
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

struct AgentOperationProgressSink {
    operations: Arc<AgentOperationRegistry>,
    operation_id: String,
    root_agent_id: Mutex<Option<String>>,
}

impl AgentOperationProgressSink {
    fn new(operations: Arc<AgentOperationRegistry>, operation_id: String) -> Self {
        Self {
            operations,
            operation_id,
            root_agent_id: Mutex::new(None),
        }
    }

    fn is_root_agent(&self, agent_id: &AgentId) -> bool {
        self.root_agent_id
            .lock()
            .map(|root| root.as_deref() == Some(agent_id.0.as_str()))
            .unwrap_or(false)
    }
}

impl LoopEventSink for AgentOperationProgressSink {
    fn on_turn_start(&self, agent_id: &AgentId, turn: u32) {
        let is_root = {
            let Ok(mut root) = self.root_agent_id.lock() else {
                return;
            };
            if root.is_none() {
                *root = Some(agent_id.0.clone());
            }
            root.as_deref() == Some(agent_id.0.as_str())
        };
        if is_root {
            self.operations.mark_running(&self.operation_id, turn);
        }
    }

    fn on_assistant_message(&self, agent_id: &AgentId, text: &str) {
        if self.is_root_agent(agent_id) {
            self.operations.update_last_text(&self.operation_id, text);
        }
    }

    fn on_assistant_reasoning(&self, _agent_id: &AgentId, _text: &str) {}

    fn on_tool_result(&self, _agent_id: &AgentId, _event: &ToolResultEvent) {}

    fn on_loop_end(&self, _agent_id: &AgentId, _summary: &LoopEndSummary) {}
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
#[path = "../../../tests/unit/serverside/mcp_server_test.rs"]
mod tests;
