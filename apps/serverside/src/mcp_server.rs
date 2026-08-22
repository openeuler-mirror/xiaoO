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

    #[cfg(test)]
    fn status(&self, operation_id: &str) -> Result<AgentOperationOutput, String> {
        let inner = self
            .inner
            .lock()
            .map_err(|_| "agent operation registry is unavailable".to_string())?;
        let record = inner
            .operations
            .get(operation_id)
            .ok_or_else(|| format!("unknown or expired operation_id `{operation_id}`"))?;
        Ok(record.output())
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
        reasoning_effort: Default::default(),
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
mod tests {
    use super::*;
    use agent_types::context::{FeatureFlags, TokenBudgetConfig};
    use async_trait::async_trait;
    use axum::body::{to_bytes, Body};
    use axum::http::Request as HttpRequest;
    use std::collections::BTreeMap;
    use tempfile::TempDir;
    use tokio::sync::Semaphore;
    use tower::ServiceExt;
    use xiaoo_shared::gateway::{
        InMemorySessionStore, SessionControlPlane, SessionRuntimeSnapshot, SessionServiceError,
    };

    struct MockSessions;

    fn mock_turn_result() -> AppTurnResult {
        AppTurnResult {
            raw_reply: "mock reply".to_string(),
            visible_reply: "mock reply".to_string(),
            messages: Vec::new(),
            prompt_tokens: 3,
            completion_tokens: 2,
            total_tokens: 5,
            estimated_input_tokens: 3,
            outcome: xiaoo_shared::gateway::TurnOutcome::Complete,
            hook_actions: Vec::new(),
        }
    }

    #[async_trait]
    impl SessionService for MockSessions {
        async fn run_turn(
            &self,
            _request: AppTurnRequest,
        ) -> Result<AppTurnResult, SessionServiceError> {
            Ok(mock_turn_result())
        }
    }

    #[async_trait]
    impl SessionControlPlane for MockSessions {}

    struct BlockingSessions {
        started: Arc<Semaphore>,
        release: Arc<Semaphore>,
    }

    #[async_trait]
    impl SessionService for BlockingSessions {
        async fn run_turn(
            &self,
            _request: AppTurnRequest,
        ) -> Result<AppTurnResult, SessionServiceError> {
            self.started.add_permits(1);
            let permit = self.release.acquire().await.expect("release semaphore");
            permit.forget();
            Ok(mock_turn_result())
        }

        async fn run_turn_with_interaction(
            &self,
            _request: AppTurnRequest,
            event_sink: Option<Arc<dyn LoopEventSink>>,
            _interaction_handle: Option<Arc<dyn agent_contracts::InteractionHandle>>,
            _channel_file_sender: Option<Arc<dyn agent_contracts::ChannelFileSender>>,
            _cancellation_token: Option<CancellationToken>,
            _tool_event_sink: Option<Arc<dyn agent_contracts::ToolEventSink>>,
        ) -> Result<AppTurnResult, SessionServiceError> {
            if let Some(sink) = event_sink {
                let root = AgentId("core".to_string());
                sink.on_turn_start(&root, 1);
                sink.on_assistant_message(&root, "working");
            }
            self.run_turn(_request).await
        }
    }

    struct TerminalErrorSessions {
        panic: bool,
    }

    #[async_trait]
    impl SessionService for TerminalErrorSessions {
        async fn run_turn(
            &self,
            _request: AppTurnRequest,
        ) -> Result<AppTurnResult, SessionServiceError> {
            if self.panic {
                panic!("intentional background task panic");
            }
            Err(SessionServiceError::CoreRun {
                message: "private backend failure".to_string(),
            })
        }
    }

    async fn wait_for_done(
        operations: &AgentOperationRegistry,
        operation_id: &str,
    ) -> AgentOperationOutput {
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let status = operations.status(operation_id).expect("operation status");
                if matches!(&status.detail, AgentOperationDetail::Done { .. }) {
                    break status;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("operation should finish")
    }

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

    #[test]
    fn agent_operation_schema_has_only_portable_integer_types() {
        let schema = serde_json::to_value(schemars::schema_for!(AgentOperationOutput))
            .expect("agent operation schema should serialize");
        let schema_text = schema.to_string();
        assert!(!schema_text.contains("uint64"), "{schema_text}");
        assert!(!schema_text.contains("uint32"), "{schema_text}");
    }

    #[test]
    fn agent_progress_keeps_only_latest_root_turn_text() {
        let operations = Arc::new(AgentOperationRegistry::default());
        let (operation_id, _cancel, _initial) = operations
            .register(
                "mcp_agent_progress".to_string(),
                PathBuf::from("/tmp"),
                true,
            )
            .expect("register operation");
        let sink = AgentOperationProgressSink::new(operations.clone(), operation_id.clone());
        let root = AgentId("core".to_string());
        let subagent = AgentId("subagent-1".to_string());

        sink.on_turn_start(&root, 1);
        sink.on_assistant_message(&root, "first partial");
        sink.on_assistant_message(&root, "first complete");
        sink.on_assistant_message(&subagent, "subagent private text");
        sink.on_assistant_reasoning(&root, "private reasoning");
        sink.on_tool_result(
            &root,
            &ToolResultEvent {
                call_id: "call-1".to_string(),
                tool_name: "bash".to_string(),
                output_preview: "private output".to_string(),
                is_error: false,
                args_preview: "private args".to_string(),
            },
        );

        let first = serde_json::to_value(operations.status(&operation_id).expect("running status"))
            .expect("serialize status");
        assert_eq!(first["snapshot"]["current_turn"], 1);
        assert_eq!(first["snapshot"]["last_text"], "first complete");
        let first_text = first.to_string();
        assert!(!first_text.contains("subagent private"), "{first_text}");
        assert!(!first_text.contains("private reasoning"), "{first_text}");
        assert!(!first_text.contains("private output"), "{first_text}");
        assert!(!first_text.contains("private args"), "{first_text}");
        assert!(!first_text.contains("bash"), "{first_text}");

        sink.on_turn_start(&root, 2);
        let before_new_text = serde_json::to_value(
            operations
                .status(&operation_id)
                .expect("second turn status"),
        )
        .expect("serialize status");
        assert_eq!(before_new_text["snapshot"]["current_turn"], 2);
        assert_eq!(before_new_text["snapshot"]["last_text"], "first complete");

        sink.on_assistant_message(&root, "second turn");
        let second =
            serde_json::to_value(operations.status(&operation_id).expect("updated status"))
                .expect("serialize status");
        assert_eq!(second["snapshot"]["last_text"], "second turn");
    }

    #[test]
    fn registry_rejects_busy_session_and_retains_latest_sixteen_results() {
        let operations = AgentOperationRegistry::default();
        let workspace = PathBuf::from("/tmp");
        let (active_id, _cancel, _initial) = operations
            .register("mcp_agent_retention".to_string(), workspace.clone(), true)
            .expect("first operation");
        let busy = operations
            .register("mcp_agent_retention".to_string(), workspace.clone(), false)
            .expect_err("active session must be busy");
        assert!(busy.contains(&active_id), "{busy}");
        operations.complete_success(&active_id, mock_turn_result());

        let mut operation_ids = vec![active_id];
        for _ in 0..COMPLETED_OPERATIONS_PER_SESSION {
            let (operation_id, _cancel, _initial) = operations
                .register("mcp_agent_retention".to_string(), workspace.clone(), false)
                .expect("next operation");
            operations.complete_success(&operation_id, mock_turn_result());
            operation_ids.push(operation_id);
        }

        assert!(operations.status(&operation_ids[0]).is_err());
        for operation_id in &operation_ids[1..] {
            assert!(operations.status(operation_id).is_ok(), "{operation_id}");
        }
    }

    #[test]
    fn reaper_removes_ten_minute_old_results_and_empty_session_indexes() {
        let operations = AgentOperationRegistry::default();
        let workspace = PathBuf::from("/tmp");
        let completed_session = "mcp_agent_expired";
        let (completed_id, _cancel, _initial) = operations
            .register(completed_session.to_string(), workspace.clone(), true)
            .expect("completed operation");
        operations.complete_success(&completed_id, mock_turn_result());
        {
            let mut inner = operations.inner.lock().expect("operation registry");
            inner
                .operations
                .get_mut(&completed_id)
                .expect("completed record")
                .completed_at = Instant::now().checked_sub(COMPLETED_OPERATION_RETENTION);
        }

        let (active_id, _cancel, _initial) = operations
            .register("mcp_agent_active".to_string(), workspace, true)
            .expect("active operation");
        assert_eq!(
            operations.reap_expired_completed(COMPLETED_OPERATION_RETENTION),
            1
        );
        assert!(operations.status(&completed_id).is_err());
        assert!(operations.status(&active_id).is_ok());
        let inner = operations.inner.lock().expect("operation registry");
        assert!(!inner.completed_by_session.contains_key(completed_session));
    }

    #[tokio::test]
    async fn agent_operation_returns_immediately_and_can_be_polled_to_done() {
        let workspace = TempDir::new().expect("workspace");
        let started = Arc::new(Semaphore::new(0));
        let release = Arc::new(Semaphore::new(0));
        let state = McpRuntimeState {
            session_service: Arc::new(BlockingSessions {
                started: started.clone(),
                release: release.clone(),
            }),
            session_store: Arc::new(InMemorySessionStore::default()),
            chatbot_workspace: workspace.path().to_path_buf(),
            agent_role: Some("diagnostician".to_string()),
            agent_operations: Arc::new(AgentOperationRegistry::default()),
        };

        let initial = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            start_mcp_agent_operation(
                &state,
                "long task".to_string(),
                None,
                Some(workspace.path().to_path_buf()),
            ),
        )
        .await
        .expect("agent submission must return promptly")
        .expect("agent submission");
        let AgentOperationOutput {
            operation_id,
            session_id,
            detail: AgentOperationDetail::Running { snapshot, .. },
            ..
        } = initial
        else {
            panic!("initial state must be running");
        };
        assert_eq!(snapshot.phase, AgentOperationPhase::Queued);

        let started_permit =
            tokio::time::timeout(std::time::Duration::from_secs(1), started.acquire())
                .await
                .expect("background operation should start")
                .expect("started semaphore");
        started_permit.forget();
        let running = state
            .agent_operations
            .status(&operation_id)
            .expect("running status");
        let running_json = serde_json::to_value(running).expect("serialize running status");
        assert_eq!(running_json["state"], "running");
        assert_eq!(running_json["snapshot"]["phase"], "running");
        assert_eq!(running_json["snapshot"]["last_text"], "working");

        let busy =
            start_mcp_agent_operation(&state, "another task".to_string(), Some(session_id), None)
                .await
                .expect_err("same session must remain busy");
        assert!(busy.contains(&operation_id), "{busy}");

        let status_poll = poll_agent_operation(&state.agent_operations, &operation_id);
        tokio::pin!(status_poll);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), status_poll.as_mut())
                .await
                .is_err(),
            "an early running poll must be held by the server"
        );
        release.add_permits(1);
        let done = tokio::time::timeout(std::time::Duration::from_secs(1), status_poll.as_mut())
            .await
            .expect("completion should wake the held poll")
            .expect("polled operation status");
        let done_json = serde_json::to_value(done).expect("serialize done status");
        assert_eq!(done_json["state"], "done");
        assert_eq!(done_json["reply"], "mock reply");
        assert_eq!(done_json["outcome"], "complete");
        assert_eq!(done_json["usage"]["total_tokens"], 5);
        assert!(done_json["error"].is_null());
    }

    #[tokio::test]
    async fn background_errors_and_panics_become_sanitized_done_results() {
        for should_panic in [false, true] {
            let workspace = TempDir::new().expect("workspace");
            let state = McpRuntimeState {
                session_service: Arc::new(TerminalErrorSessions {
                    panic: should_panic,
                }),
                session_store: Arc::new(InMemorySessionStore::default()),
                chatbot_workspace: workspace.path().to_path_buf(),
                agent_role: None,
                agent_operations: Arc::new(AgentOperationRegistry::default()),
            };
            let initial = start_mcp_agent_operation(
                &state,
                "failing task".to_string(),
                None,
                Some(workspace.path().to_path_buf()),
            )
            .await
            .expect("operation should be accepted");
            let operation_id = initial.operation_id.clone();
            let done = wait_for_done(&state.agent_operations, &operation_id).await;
            let done_json = serde_json::to_value(done).expect("serialize failed status");
            assert_eq!(done_json["state"], "done");
            assert_eq!(done_json["outcome"], "failed");
            assert!(done_json["usage"].is_null());
            let error = done_json["error"].as_str().expect("sanitized error");
            assert!(error.contains("inspect daemon logs"), "{error}");
            assert!(!error.contains("private backend failure"), "{error}");
            assert!(!error.contains("intentional background"), "{error}");
        }
    }

    fn session_record(
        session_id: &str,
        endpoint: McpEndpoint,
        workspace: PathBuf,
        runtime_profile_id: Option<&str>,
    ) -> SessionRecord {
        SessionRecord {
            session_id: session_id.to_string(),
            conversation_id: "conversation".to_string(),
            sender_id: "sender".to_string(),
            entry: GatewayEntryContext {
                kind: Some(GatewayEntryKind::Mcp),
                instance_id: Some(endpoint.instance_id().to_string()),
                runtime_profile_id: runtime_profile_id.map(ToString::to_string),
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
            None,
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
                agent_role: None,
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

    fn parse_mcp_response(body: &[u8]) -> serde_json::Value {
        if let Ok(value) = serde_json::from_slice(body) {
            return value;
        }
        let text = String::from_utf8_lossy(body);
        let data = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("data: ")
                    .or_else(|| line.strip_prefix("data:"))
            })
            .map(str::trim)
            .find(|data| !data.is_empty())
            .unwrap_or_else(|| panic!("response is neither JSON nor SSE data: {text}"));
        serde_json::from_str(data)
            .unwrap_or_else(|error| panic!("invalid MCP SSE JSON ({error}): {text}"))
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
        let initialize_body = to_bytes(initialized.into_body(), 1024 * 1024)
            .await
            .expect("initialize body");
        let initialize_body =
            String::from_utf8(initialize_body.to_vec()).expect("UTF-8 initialize body");
        assert!(
            initialize_body.contains("xiaoo-chatbot"),
            "{initialize_body}"
        );
        assert!(
            initialize_body.contains("Web-only chatbot"),
            "{initialize_body}"
        );

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
        assert!(body.contains("web_search and webfetch"), "{body}");
        assert!(body.contains("no file, shell"), "{body}");
        assert!(!body.contains("read files"), "{body}");
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
    async fn agent_endpoint_lists_agent_tools_and_requires_workspace_for_new_call() {
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
        let initialize_body = to_bytes(initialized.into_body(), 1024 * 1024)
            .await
            .expect("initialize body");
        let initialize_body =
            String::from_utf8(initialize_body.to_vec()).expect("UTF-8 initialize body");
        assert!(initialize_body.contains("xiaoo-agent"), "{initialize_body}");
        assert!(
            initialize_body.contains("Full local agent"),
            "{initialize_body}"
        );

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
        assert!(body.contains("\"name\":\"agent_status\""), "{body}");
        assert!(body.contains("file, shell, skill, plugin"), "{body}");
        assert!(body.contains("absolute existing workspace"), "{body}");
        assert!(body.contains("poll agent_status"), "{body}");
        assert!(!body.contains("\"name\":\"chat\""), "{body}");

        let called = router
            .clone()
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

        let submitted = router
            .clone()
            .oneshot(request(
                4,
                "tools/call",
                serde_json::json!({
                    "name": "agent",
                    "arguments": {
                        "message": "hello",
                        "workspace": workspace.path().to_string_lossy()
                    }
                }),
            ))
            .await
            .expect("agent submission response");
        let submitted_body = to_bytes(submitted.into_body(), 1024 * 1024)
            .await
            .expect("submission body");
        let submitted_json = parse_mcp_response(&submitted_body);
        let submitted_output = &submitted_json["result"]["structuredContent"];
        assert_eq!(submitted_output["state"], "running");
        assert_eq!(submitted_output["snapshot"]["phase"], "queued");
        let operation_id = submitted_output["operation_id"]
            .as_str()
            .expect("operation ID")
            .to_string();
        assert!(operation_id.starts_with("mcp_op_"), "{operation_id}");

        let mut final_status = None;
        for id in 5..25 {
            let polled = router
                .clone()
                .oneshot(request(
                    id,
                    "tools/call",
                    serde_json::json!({
                        "name": "agent_status",
                        "arguments": {"operation_id": operation_id.as_str()}
                    }),
                ))
                .await
                .expect("agent status response");
            let polled_body = to_bytes(polled.into_body(), 1024 * 1024)
                .await
                .expect("status body");
            let polled_json = parse_mcp_response(&polled_body);
            let status = polled_json["result"]["structuredContent"].clone();
            if status["state"] == "done" {
                final_status = Some(status);
                break;
            }
            tokio::task::yield_now().await;
        }
        let final_status = final_status.expect("operation should become done");
        assert_eq!(final_status["reply"], "mock reply");
        assert_eq!(final_status["outcome"], "complete");
    }
}
