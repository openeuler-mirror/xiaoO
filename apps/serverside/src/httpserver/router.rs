use crate::channel_management::ChannelManager;
use crate::channels::{AdapterResponse, ChannelError, ChannelResult, ChannelRuntime};
use crate::cron::scheduler::{CronScheduler, TriggerError};
use crate::httpserver::channel_ingress::GatewayChannelIngressError;
use crate::httpserver::channel_runtime::{ChannelMessageProcessingError, ChannelRuntimeProcessor};
use crate::httpserver::rate_limit::RateLimitConfig;
use crate::httpserver::sse_sink::{
    sse_stream_from_receiver, SseLoopEventSink, SseStreamEvent, SseToolEventSink,
};
use crate::httpserver::GatewayServiceError;
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
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tower_http::cors::CorsLayer;
use tracing::warn;
use xiaoo_api::interaction::{InteractionHandle, InteractionRequest, InteractionResponse};
use xiaoo_shared::daemon_protocol::response::{
    ChannelCatalogResponse, CronCatalogResponse, CronRunResponse, DaemonService,
    GatewayCapabilitiesResponse, GatewayErrorResponse, GatewayFeatureCapabilities,
    GatewayHealthResponse, GatewayHealthStatus, GatewayTransport, RuntimeCatalogResponse,
    RuntimeCheckoutResponse, RuntimeCheckpointCatalogItem, RuntimeCheckpointCatalogResponse,
    RuntimeCheckpointResponse, RuntimeCheckpointSnapshotDeleteResponse,
    RuntimeExecInterruptedResponse, RuntimeExecResponse, RuntimeExportFormat,
    RuntimeExportResponse, RuntimeLifecycleStatus, RuntimePauseResponse, RuntimeReadFileResponse,
    RuntimeRecordResponse, RuntimeResumeResponse, RuntimeWriteFileResponse, SandboxCatalogItem,
    SandboxCatalogResponse, SandboxLifecycleStatus, SandboxResourceAllocation,
};
use xiaoo_shared::gateway::{is_daemon_principal, SessionControlPlane, SessionService};
use xiaoo_shared::plan::{
    PlanComputingLoopSink, PlanForwarder, SubagentMetaComputingLoopSink, SubagentMetaForwarder,
};
use xiaoo_shared::session_diff::{
    DiffComputingLoopSink, DiffComputingToolSink, SessionDiffForwarder, SessionDiffTracker,
};

#[derive(Clone)]
pub struct GatewayAppState {
    session_service: Arc<dyn SessionService>,
    session_control_plane: Option<Arc<dyn SessionControlPlane>>,
    channel_runtimes: Arc<HashMap<String, ChannelRuntime>>,
    channel_processor: ChannelRuntimeProcessor,
    remote_interactions: Arc<RemoteInteractionStore>,
    action_sink: Option<Arc<xiaoo_shared::gateway::DaemonHookActionSink>>,
    cron_scheduler: Option<Arc<CronScheduler>>,
    channel_manager: Option<Arc<ChannelManager>>,
    session_diff_trackers: SessionDiffTrackerMap,
}

/// Per-session `SessionDiffTracker` map. Shared between the request path
/// (which creates/reads trackers) and the background sweep task (which
/// evicts trackers for sessions that no longer have a runtime handle).
type SessionDiffTrackerMap =
    Arc<std::sync::RwLock<HashMap<String, Arc<std::sync::Mutex<SessionDiffTracker>>>>>;

/// How often the background sweep checks for stale diff trackers.
const DIFF_TRACKER_SWEEP_INTERVAL_SECS: u64 = 300;

/// Per-session timeout for the liveness probe in `sweep_stale_diff_trackers`.
/// Bounds the worst case where a single unresponsive `resume_session` call
/// (lock contention, I/O stall, downstream hang) would otherwise block the
/// sweep loop indefinitely, letting stale trackers accumulate.
const DIFF_TRACKER_SWEEP_PROBE_TIMEOUT_SECS: u64 = 5;

impl GatewayAppState {
    pub fn new(session_service: Arc<dyn SessionService>) -> Self {
        Self {
            channel_processor: ChannelRuntimeProcessor::new(session_service.clone()),
            session_service,
            session_control_plane: None,
            channel_runtimes: Arc::new(HashMap::new()),
            remote_interactions: Arc::new(RemoteInteractionStore::default()),
            action_sink: None,
            cron_scheduler: None,
            channel_manager: None,
            session_diff_trackers: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }

    pub fn with_control_plane(
        session_service: Arc<dyn SessionService>,
        session_control_plane: Arc<dyn SessionControlPlane>,
    ) -> Self {
        let mut state = Self::new(session_service);
        state.session_control_plane = Some(session_control_plane.clone());
        state.action_sink = Some(Arc::new(xiaoo_shared::gateway::DaemonHookActionSink::new(
            session_control_plane,
        )));
        state
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
            action_sink: None,
            cron_scheduler: None,
            channel_manager: None,
            session_diff_trackers: Arc::new(std::sync::RwLock::new(HashMap::new())),
        })
    }

    pub fn with_channel_runtimes_and_control_plane(
        session_service: Arc<dyn SessionService>,
        session_control_plane: Arc<dyn SessionControlPlane>,
        runtimes: Vec<ChannelRuntime>,
    ) -> ChannelResult<Self> {
        let mut state = Self::with_channel_runtimes(session_service, runtimes)?;
        state.action_sink = Some(Arc::new(xiaoo_shared::gateway::DaemonHookActionSink::new(
            session_control_plane.clone(),
        )));
        state.session_control_plane = Some(session_control_plane);
        Ok(state)
    }

    fn set_cron_scheduler(&mut self, cron_scheduler: Option<Arc<CronScheduler>>) {
        self.cron_scheduler = cron_scheduler;
    }

    fn set_channel_interaction_timeout(&mut self, interaction_timeout_secs: u64) {
        self.channel_processor = match self.channel_manager.as_ref() {
            Some(manager) => ChannelRuntimeProcessor::with_monitor(
                self.session_service.clone(),
                interaction_timeout_secs,
                manager.clone(),
            ),
            None => ChannelRuntimeProcessor::with_timeout(
                self.session_service.clone(),
                interaction_timeout_secs,
            ),
        };
    }

    fn set_channel_manager(&mut self, channel_manager: Arc<ChannelManager>) {
        self.channel_processor = ChannelRuntimeProcessor::with_monitor(
            self.session_service.clone(),
            600,
            channel_manager.clone(),
        );
        self.channel_manager = Some(channel_manager);
    }

    /// Look up (or lazily create) the per-session [`SessionDiffTracker`].
    /// The `workspace` argument is only used when the tracker needs to be
    /// created; subsequent calls for the same `session_id` reuse the existing
    /// tracker so file baselines accumulate across turns.
    pub fn diff_tracker_for(
        &self,
        session_id: &str,
        workspace: std::path::PathBuf,
    ) -> Arc<std::sync::Mutex<SessionDiffTracker>> {
        if let Some(tracker) = self
            .session_diff_trackers
            .read()
            .ok()
            .and_then(|g| g.get(session_id).cloned())
        {
            return tracker;
        }
        let mut write_guard = write_trackers(&self.session_diff_trackers, " for insert");
        if let Some(existing) = write_guard.get(session_id) {
            return Arc::clone(existing);
        }
        let tracker = Arc::new(std::sync::Mutex::new(SessionDiffTracker::new(workspace)));
        write_guard.insert(session_id.to_string(), Arc::clone(&tracker));
        tracker
    }

    /// Drop the per-session [`SessionDiffTracker`] associated with `session_id`.
    /// Called when a session is closed so the daemon does not leak tracker
    /// state (and its accumulated per-call maps) across long-running processes.
    /// Returns `true` when a tracker was actually present.
    pub fn evict_diff_tracker(&self, session_id: &str) -> bool {
        let mut write_guard = write_trackers(&self.session_diff_trackers, " for evict");
        write_guard.remove(session_id).is_some()
    }
}

/// Spawn a background task that periodically evicts diff trackers for
/// sessions whose runtime handle no longer exists (e.g. the client
/// disconnected without calling `/close`, and the session was later
/// hibernated by the idle reaper). This is a safety net complementing the
/// explicit `evict_diff_tracker` call in the `force_close_session` path —
/// it catches cleanup paths the router does not directly observe.
///
/// The sweep calls `resume_session` for each tracked session id; a `None`
/// result means the session handle is gone (closed or hibernated) and the
/// tracker should be evicted.
fn spawn_diff_tracker_sweep(
    trackers: SessionDiffTrackerMap,
    session_control_plane: Arc<dyn SessionControlPlane>,
) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            DIFF_TRACKER_SWEEP_INTERVAL_SECS,
        ));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            interval.tick().await;
            sweep_stale_diff_trackers(&trackers, &session_control_plane).await;
        }
    });
}

async fn sweep_stale_diff_trackers(
    trackers: &SessionDiffTrackerMap,
    session_control_plane: &Arc<dyn SessionControlPlane>,
) {
    let session_ids: Vec<String> = {
        let guard = match trackers.read() {
            Ok(guard) => guard,
            Err(poisoned) => {
                tracing::error!(
                    "session_diff_trackers lock poisoned during sweep; \
                     recovering and skipping this cycle"
                );
                poisoned.into_inner()
            }
        };
        guard.keys().cloned().collect()
    };
    if session_ids.is_empty() {
        return;
    }
    let mut evicted = 0usize;
    for session_id in &session_ids {
        let probe = tokio::time::timeout(
            std::time::Duration::from_secs(DIFF_TRACKER_SWEEP_PROBE_TIMEOUT_SECS),
            session_control_plane.resume_session(session_id),
        );
        match probe.await {
            Ok(Ok(None)) => {
                evict_tracker_from_map(trackers, session_id);
                evicted += 1;
            }
            Ok(Ok(Some(_))) => {} // Session still active; keep tracker.
            Ok(Err(error)) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to check session liveness during diff tracker sweep; \
                     keeping tracker"
                );
            }
            Err(_elapsed) => {
                tracing::warn!(
                    session_id = %session_id,
                    timeout_secs = DIFF_TRACKER_SWEEP_PROBE_TIMEOUT_SECS,
                    "resume_session timed out during diff tracker sweep; \
                     keeping tracker this cycle"
                );
            }
        }
    }
    if evicted > 0 {
        tracing::info!(
            evicted,
            total_checked = session_ids.len(),
            "swept stale diff trackers"
        );
    }
}

/// Remove a single tracker entry from the map (best-effort; ignores
/// lock poisoning by recovering).
fn evict_tracker_from_map(trackers: &SessionDiffTrackerMap, session_id: &str) {
    let mut write_guard = write_trackers(trackers, " during sweep evict");
    write_guard.remove(session_id);
}

/// Acquire a write lock on the shared diff-tracker map, recovering from
/// poisoning so the daemon keeps operating. `context` is appended to the
/// poison log so the call site can be identified in logs.
fn write_trackers<'a>(
    trackers: &'a SessionDiffTrackerMap,
    context: &str,
) -> std::sync::RwLockWriteGuard<'a, HashMap<String, Arc<std::sync::Mutex<SessionDiffTracker>>>> {
    match trackers.write() {
        Ok(guard) => guard,
        Err(poisoned) => {
            tracing::error!("session_diff_trackers lock poisoned{context}; recovering");
            poisoned.into_inner()
        }
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

    /// Drop the pending `oneshot::Sender` for `session_id` without sending
    /// a response. Called by `RemoteSseInteractionHandle::abort_pending`
    /// when the gateway's defensive outer `interaction_timeout` fires
    /// before the user replied: the `ask` future (which held the
    /// `Receiver`) has already been dropped by `tokio::select!`, so this
    /// just removes the orphaned `Sender` so a late user reply via
    /// [`answer`](Self::answer) returns `false` (instead of finding a
    /// stale entry whose `Sender` would silently fail to deliver).
    async fn cancel(&self, session_id: &str) {
        self.pending.lock().await.remove(session_id);
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
            // Pending entry aborted (e.g. the turn was cancelled): the
            // `Sender` was dropped without a reply, so the pending tool call
            // gets a deny-style result and can wind down.
            Err(_) => InteractionResponse::unanswered(request),
        }
    }

    /// Releases the orphaned pending entry via `store.cancel` so a late
    /// user reply (via the SSE `/interaction/respond` route) returns `false`
    /// instead of being silently swallowed by a stale `Sender`.
    async fn abort_pending(&self, _request: &InteractionRequest) {
        self.store.cancel(&self.session_id).await;
    }
}

fn runtime_record_response(record: xiaoo_shared::RuntimeRecord) -> RuntimeRecordResponse {
    RuntimeRecordResponse {
        runtime_id: record.runtime_id,
        conversation_id: record.conversation_id,
        sender_id: record.sender_id,
        status: match record.status {
            xiaoo_shared::gateway::SessionLifecycleStatus::Idle => RuntimeLifecycleStatus::Idle,
            xiaoo_shared::gateway::SessionLifecycleStatus::Running => {
                RuntimeLifecycleStatus::Running
            }
            xiaoo_shared::gateway::SessionLifecycleStatus::Paused => RuntimeLifecycleStatus::Paused,
            xiaoo_shared::gateway::SessionLifecycleStatus::Failed => RuntimeLifecycleStatus::Failed,
            xiaoo_shared::gateway::SessionLifecycleStatus::Closed => RuntimeLifecycleStatus::Closed,
        },
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

fn runtime_checkpoint_response(
    result: xiaoo_shared::RuntimeCheckpointResult,
) -> RuntimeCheckpointResponse {
    RuntimeCheckpointResponse {
        checkpoint_id: result.checkpoint_id,
        runtime: runtime_record_response(result.runtime),
        parent_checkpoint_id: result.parent_checkpoint_id,
        created_at_ms: result.created_at_ms,
        metadata: result.metadata,
        name: result.name,
    }
}

fn runtime_checkout_response(
    result: xiaoo_shared::RuntimeCheckoutResult,
) -> RuntimeCheckoutResponse {
    RuntimeCheckoutResponse {
        checkpoint_id: result.checkpoint_id,
        source_runtime_id: result.source_runtime_id,
        runtime: runtime_record_response(result.runtime),
    }
}

fn runtime_pause_response(result: xiaoo_shared::RuntimePauseResult) -> RuntimePauseResponse {
    RuntimePauseResponse {
        runtime: runtime_record_response(result.runtime),
        checkpoint_id: result.checkpoint_id,
        created_at_ms: result.created_at_ms,
        metadata: result.metadata,
        name: result.name,
    }
}

fn runtime_resume_response(result: xiaoo_shared::RuntimeResumeResult) -> RuntimeResumeResponse {
    RuntimeResumeResponse {
        runtime: runtime_record_response(result.runtime),
    }
}

fn runtime_checkpoint_snapshot_delete_response(
    result: xiaoo_shared::RuntimeCheckpointSnapshotDeleteResult,
) -> RuntimeCheckpointSnapshotDeleteResponse {
    RuntimeCheckpointSnapshotDeleteResponse {
        checkpoint_id: result.checkpoint_id,
        runtime_id: result.runtime_id,
        provider: result.provider,
        provider_snapshot_id: result.provider_snapshot_id,
        provider_snapshot_names: result.provider_snapshot_names,
        deleted_provider_snapshot: result.deleted_provider_snapshot,
        deleted_at_ms: result.deleted_at_ms,
    }
}

fn runtime_exec_response(result: xiaoo_shared::RuntimeExecResult) -> RuntimeExecResponse {
    RuntimeExecResponse {
        stdout_base64: result.stdout_base64,
        stderr_base64: result.stderr_base64,
        exit_code: result.exit_code,
        timed_out: result.timed_out,
    }
}

fn runtime_read_file_response(
    result: xiaoo_shared::RuntimeReadFileResult,
) -> RuntimeReadFileResponse {
    RuntimeReadFileResponse {
        content_base64: result.content_base64,
    }
}

fn runtime_write_file_response(
    result: xiaoo_shared::RuntimeWriteFileResult,
) -> RuntimeWriteFileResponse {
    RuntimeWriteFileResponse {
        path: result.path,
        created: result.created,
    }
}

fn runtime_checkpoint_catalog_item(
    checkpoint: xiaoo_shared::RuntimeCheckpointSummary,
) -> RuntimeCheckpointCatalogItem {
    RuntimeCheckpointCatalogItem {
        checkpoint_id: checkpoint.checkpoint_id,
        runtime_id: checkpoint.runtime_id,
        parent_checkpoint_id: checkpoint.parent_checkpoint_id,
        created_at_ms: checkpoint.created_at_ms,
        metadata: checkpoint.metadata,
        name: checkpoint.name,
        has_provider_snapshot: checkpoint.has_provider_snapshot,
    }
}

fn sandbox_catalog_item(sandbox: xiaoo_shared::backend::BackendInfo) -> SandboxCatalogItem {
    let status = match xiaoo_shared::backend::backend_state_label(sandbox.state) {
        "unknown" => SandboxLifecycleStatus::Unknown,
        "creating" => SandboxLifecycleStatus::Creating,
        "active" => SandboxLifecycleStatus::Active,
        "pausing" => SandboxLifecycleStatus::Pausing,
        "paused" => SandboxLifecycleStatus::Paused,
        "loading" => SandboxLifecycleStatus::Loading,
        "deleting" => SandboxLifecycleStatus::Deleting,
        "deleted" => SandboxLifecycleStatus::Deleted,
        "failed" => SandboxLifecycleStatus::Failed,
        state => unreachable!("shared returned an unknown backend state: {state}"),
    };
    SandboxCatalogItem {
        sandbox_id: sandbox.backend_id,
        provider: sandbox.provider,
        instance_id: sandbox.instance_id,
        status,
        workspace_root: sandbox.workspace_root,
        endpoint_kind: xiaoo_shared::backend::backend_endpoint_kind(sandbox.endpoint)
            .map(str::to_string),
        resources: SandboxResourceAllocation {
            vcpu_count: sandbox.resources.vcpu_count,
            memory_mb: sandbox.resources.memory_mb,
            disk_mb: sandbox.resources.disk_mb,
        },
        runtime_ids: sandbox.session_ids,
        expires_at_ms: sandbox.expires_at_ms,
        parent_sandbox_id: sandbox.lineage.parent_backend_id,
        child_sandbox_ids: sandbox.lineage.children_backend_ids,
        forked_from_snapshot_id: sandbox.lineage.forked_from_snapshot_id,
        forked_at_ms: sandbox.lineage.forked_at_ms,
    }
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

pub fn create_router_with_channel_runtimes_control_plane_and_timeout_and_auth(
    session_service: Arc<dyn SessionService>,
    session_control_plane: Arc<dyn SessionControlPlane>,
    runtimes: Vec<ChannelRuntime>,
    interaction_timeout_secs: u64,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
    cron_scheduler: Option<Arc<CronScheduler>>,
    channel_manager: Arc<ChannelManager>,
) -> ChannelResult<Router> {
    let mut state = GatewayAppState::with_channel_runtimes_and_control_plane(
        session_service,
        session_control_plane.clone(),
        runtimes,
    )?;
    state.set_channel_manager(channel_manager);
    state.set_channel_interaction_timeout(interaction_timeout_secs);
    state.set_cron_scheduler(cron_scheduler);
    spawn_diff_tracker_sweep(state.session_diff_trackers.clone(), session_control_plane);
    Ok(create_router_from_state(state, bearer_auth, rate_limit))
}

fn create_router_from_state(
    state: GatewayAppState,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
) -> Router {
    let protected_runtime_routes = apply_http_bearer_auth(
        Router::new()
            .route("/api/v1/runtimes", get(handle_runtime_catalog))
            .route("/api/v1/sandboxes", get(handle_sandbox_catalog))
            .route(
                "/api/v1/runtimes/checkpoints",
                get(handle_runtime_checkpoint_catalog),
            )
            .route("/api/v1/runtimes/open", post(handle_session_open))
            .route("/api/v1/runtimes/input", post(handle_session_input))
            .route(
                "/api/v1/runtimes/interaction",
                post(handle_session_interaction),
            )
            .route("/api/v1/runtimes/cancel", post(handle_session_cancel))
            .route("/api/v1/runtimes/close", post(handle_session_close))
            .route("/api/v1/runtimes/heartbeat", post(handle_session_heartbeat))
            .route("/api/v1/runtimes/detach", post(handle_session_detach))
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
            )
            .route("/api/v1/runtimes/export", post(handle_session_export))
            .route("/api/v1/cron/jobs", get(handle_cron_catalog))
            .route("/api/v1/cron/run", post(handle_cron_run))
            .route("/api/v1/channels", get(handle_channel_catalog))
            .route("/api/v1/channels/test", post(handle_channel_test)),
        bearer_auth.clone(),
    );

    let router = Router::new()
        .route("/api/v1/health", get(health_check))
        .route("/api/v1/capabilities", get(capabilities))
        .route(
            "/api/v1/channels/:channel_id/events",
            post(handle_channel_events),
        )
        .merge(protected_runtime_routes)
        // The Harness web UI runs on a different local origin (usually :3080)
        // and calls this daemon directly from the browser.
        .layer(CorsLayer::very_permissive())
        .with_state(Arc::new(state));

    match rate_limit.and_then(|c| c.governor_layer()) {
        Some(layer) => router.layer(layer),
        None => router,
    }
}

pub fn create_router_with_control_plane_and_auth(
    session_service: Arc<dyn SessionService>,
    session_control_plane: Arc<dyn SessionControlPlane>,
    bearer_auth: Option<HttpBearerAuthConfig>,
    rate_limit: Option<RateLimitConfig>,
    cron_scheduler: Option<Arc<CronScheduler>>,
    channel_manager: Arc<ChannelManager>,
) -> Router {
    let mut state =
        GatewayAppState::with_control_plane(session_service, session_control_plane.clone());
    state.set_cron_scheduler(cron_scheduler);
    state.set_channel_manager(channel_manager);
    spawn_diff_tracker_sweep(state.session_diff_trackers.clone(), session_control_plane);
    create_router_from_state(state, bearer_auth, rate_limit)
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
        status: GatewayHealthStatus::Ok,
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn capabilities() -> Json<GatewayCapabilitiesResponse> {
    Json(GatewayCapabilitiesResponse {
        service: DaemonService::XiaooDaemon,
        version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: xiaoo_shared::daemon_protocol::PROTOCOL_VERSION,
        minimum_client_protocol_version: xiaoo_shared::daemon_protocol::PROTOCOL_VERSION,
        transport: GatewayTransport::HttpSse,
        runtime_api: vec![
            "list",
            "list_checkpoints",
            "open",
            "input",
            "interaction",
            "cancel",
            "close",
            "heartbeat",
            "detach",
            "checkpoint",
            "checkpoint_delete_snapshot",
            "pause",
            "resume",
            "checkout",
            "exec",
            "read_file",
            "write_file",
            "export",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        sse_events: vec![
            "turn_start",
            "text_delta",
            "thinking_delta",
            "tool_result",
            "tool_file_change",
            "plan_update",
            "subagent_spawn",
            "tool_call",
            "loop_end",
            "interaction_requested",
            "done",
            "error",
            "cancelled",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        interaction_kinds: ["confirm", "text_input", "choice"]
            .into_iter()
            .map(str::to_string)
            .collect(),
        features: GatewayFeatureCapabilities {
            session_persistence: false,
            runtime_leases: true,
            checkpoints: true,
            file_operations: true,
            file_change_summary: true,
            file_change_patch: false,
            sse_resume: false,
        },
        management: crate::management_capabilities::management_capabilities(),
    })
}

/// Reject HTTP requests whose `client_id` claims the reserved daemon-internal
/// prefix (`daemon:`). Daemon-internal callers (cron / hook / channel ingress)
/// never traverse the HTTP router — they call the trait methods in-process —
/// so any HTTP request claiming the prefix is a forged attempt to bypass the
/// lease-holder check. Returns `Ok(())` for absent / non-prefixed ids, or
/// `Err(400 response)` when the prefix is forged.
fn reject_forged_daemon_principal(client_id: Option<&str>) -> Result<(), Response> {
    if let Some(cid) = client_id.filter(|s| is_daemon_principal(s)) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(GatewayErrorResponse {
                error: format!(
                    "client_id prefix `daemon:` is reserved for daemon-internal callers; \
                     HTTP clients must not claim it (got `{cid}`)"
                ),
            }),
        )
            .into_response());
    }
    Ok(())
}

/// Lease guard shared by mutating RPC handlers. Returns `Ok(())` when the
/// caller may proceed (current holder, no lease in place, or no control plane
/// configured — matching the gradual-rollout policy), or `Err(response)` (409
/// / 401 already mapped via `map_session_error`) on failure. Folds in
/// [`reject_forged_daemon_principal`] so call sites don't call it separately.
async fn require_lease_holder(
    state: &GatewayAppState,
    session_id: &str,
    client_id: Option<&str>,
) -> Result<(), Response> {
    reject_forged_daemon_principal(client_id)?;
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return Ok(());
    };
    control_plane
        .assert_lease_holder(session_id, client_id)
        .await
        .map_err(map_session_error)
}

/// Re-check the lease inside the spawned turn task and emit an SSE `Error`
/// event on failure. Returns `Break` when the caller should return (lease no
/// longer held) or `Continue` to proceed. Between the router-layer check and
/// the spawned task reaching this point another client could have taken over
/// the lease — without this re-check the daemon would execute a turn for the
/// wrong client.
async fn recheck_lease_or_emit_sse_error(
    state: &GatewayAppState,
    tx: &tokio::sync::mpsc::UnboundedSender<SseStreamEvent>,
    session_id: &str,
    client_id: Option<&str>,
) -> std::ops::ControlFlow<()> {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return std::ops::ControlFlow::Continue(());
    };
    if let Err(error) = control_plane
        .assert_lease_holder(session_id, client_id)
        .await
    {
        let _ = tx.send(SseStreamEvent::Error {
            error: error.to_string(),
        });
        return std::ops::ControlFlow::Break(());
    }
    std::ops::ControlFlow::Continue(())
}

/// Resolve the workspace path to use for the per-session diff tracker.
///
/// Priority:
/// 1. The session record's `workspace_root` (authoritative — set by the
///    resolver when the session was opened, reflects the actual workspace
///    the agent's tools operate on).
/// 2. The client-supplied `payload.workspace` hint (used only when the
///    control plane is unavailable or the session handle is gone).
/// 3. `"."` (last resort — baseline reads will miss and the tracker
///    degrades to args estimation; a warning is logged).
async fn resolve_session_workspace(
    state: &GatewayAppState,
    session_id: &str,
    payload_workspace: &Option<std::path::PathBuf>,
) -> std::path::PathBuf {
    if let Some(control_plane) = state.session_control_plane.as_ref() {
        match control_plane.resume_session(session_id).await {
            Ok(Some(record)) => {
                return record.runtime.workspace_root;
            }
            Ok(None) => {
                tracing::debug!(
                    session_id = %session_id,
                    "session handle not found for workspace resolution; \
                     falling back to payload workspace",
                );
            }
            Err(error) => {
                tracing::warn!(
                    session_id = %session_id,
                    error = %error,
                    "failed to resume session for workspace resolution; \
                     falling back to payload workspace",
                );
            }
        }
    }
    if let Some(workspace) = payload_workspace {
        return workspace.clone();
    }
    tracing::warn!(
        session_id = %session_id,
        "no authoritative workspace and no payload hint; \
         diff tracker will use current directory",
    );
    std::path::PathBuf::from(".")
}

async fn handle_session_open(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeOpenRequest>,
) -> Response {
    // Reject forged daemon-internal principals at the HTTP edge.
    if let Err(response) = reject_forged_daemon_principal(payload.client_id.as_deref()) {
        return response;
    }
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
    // Only the current lease holder may submit turns. Anonymous callers
    // (no `client_id`) bypass the check unless `XIAOO_ENFORCE_LEASE=on`.
    if let Err(response) =
        require_lease_holder(&state, &session_id, payload.client_id.as_deref()).await
    {
        return response;
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SseStreamEvent>();
    let sink = Arc::new(SseLoopEventSink::new(tx.clone()));
    // Resolve the session's workspace root from the session record
    // (authoritative — set by the resolver when the session was opened) so
    // the diff tracker reads the right files and displays workspace-relative
    // paths. Falls back to the client-supplied `payload.workspace` when the
    // control plane is unavailable, and finally to `"."` with a warning so
    // the tracker can still operate (baseline reads will simply miss and
    // degrade to args estimation).
    let workspace = resolve_session_workspace(&state, &session_id, &payload.workspace).await;
    let diff_tracker = state.diff_tracker_for(&session_id, workspace);
    let diff_forwarder = Arc::new(crate::httpserver::sse_sink::SseDeltaForwarder::new(
        tx.clone(),
    ));
    let plan_forwarder = Arc::new(crate::httpserver::sse_sink::SsePlanForwarder::new(
        tx.clone(),
    ));
    let subagent_forwarder = Arc::new(crate::httpserver::sse_sink::SseSubagentMetaForwarder::new(
        tx.clone(),
    ));
    // Compose the per-turn sink chain. The order (outermost first) is:
    // `SubagentMeta` -> `Plan` -> `Diff` -> `SseLoopEventSink`. Each layer
    // intercepts `on_tool_result` to compute its derived payload and forwards
    // to the inner sink. Tool-lifecycle events (Running) go through the
    // separately-injected `DiffComputingToolSink` baked into the runtime's
    // `bindings.tool_event_sink`.
    let diff_loop_sink: Arc<dyn xiaoo_api::events::LoopEventSink> =
        Arc::new(DiffComputingLoopSink::new(
            Arc::clone(&sink) as Arc<dyn xiaoo_api::events::LoopEventSink>,
            Arc::clone(&diff_tracker),
            diff_forwarder as Arc<dyn SessionDiffForwarder>,
        ));
    let plan_loop_sink: Arc<dyn xiaoo_api::events::LoopEventSink> = Arc::new(
        PlanComputingLoopSink::new(diff_loop_sink, plan_forwarder as Arc<dyn PlanForwarder>),
    );
    let composed_loop_sink: Arc<dyn xiaoo_api::events::LoopEventSink> =
        Arc::new(SubagentMetaComputingLoopSink::new(
            plan_loop_sink,
            subagent_forwarder as Arc<dyn SubagentMetaForwarder>,
        ));
    let diff_tool_sink: Arc<dyn xiaoo_api::events::ToolEventSink> =
        Arc::new(SseToolEventSink::with_inner(
            tx.clone(),
            Arc::new(DiffComputingToolSink::new(Arc::clone(&diff_tracker)))
                as Arc<dyn xiaoo_api::events::ToolEventSink>,
        ));
    let interaction_handle = Arc::new(RemoteSseInteractionHandle {
        session_id: session_id.clone(),
        tx: tx.clone(),
        store: state.remote_interactions.clone(),
    });
    let session_service = state.session_service.clone();
    let conversation_id = payload.conversation_id.clone();

    tokio::spawn(async move {
        // Re-check the lease before the turn starts: another client could
        // have taken over between the router-layer check and here. The
        // SessionActor's pending_turns pop-time check is a final defense
        // for turns queued longer than the lease lifetime.
        if recheck_lease_or_emit_sse_error(&state, &tx, &session_id, payload.client_id.as_deref())
            .await
            .is_break()
        {
            return;
        }
        match session_service
            .run_turn_with_interaction(
                payload,
                Some(composed_loop_sink),
                Some(interaction_handle),
                None,
                None,
                Some(diff_tool_sink),
            )
            .await
        {
            Ok(result) => {
                let summary = sink.take_loop_summary();
                let filtered_messages = filter_messages_for_display(&result.messages);
                // Execute plugin-requested actions daemon-side (e.g.
                // open_session for create/switch). The router owns the
                // DaemonHookActionSink because it has access to the session
                // control plane; CoreBackedSessionService's action_sink is
                // None (avoids the Arc self-reference chicken-and-egg).
                let actions = if result.hook_actions.is_empty() {
                    Vec::new()
                } else {
                    match state.action_sink.as_ref() {
                        Some(sink) => sink.execute_on_daemon(result.hook_actions).await,
                        None => result.hook_actions,
                    }
                };
                let _ = tx.send(SseStreamEvent::Done {
                    reply: result.visible_reply.clone(),
                    raw_reply: result.raw_reply,
                    conversation_id,
                    session_id,
                    turn_count: summary.as_ref().map_or(0, |s| s.turn_count),
                    total_tokens: result.total_tokens as usize,
                    prompt_tokens: result.prompt_tokens,
                    completion_tokens: result.completion_tokens,
                    cached_tokens: result.cached_tokens,
                    estimated_input_tokens: result.estimated_input_tokens,
                    messages: filtered_messages,
                    stop_reason: summary.map(|s| s.stop_reason).unwrap_or_default(),
                    actions,
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
    // Lease check: only the holder may answer an interaction prompt —
    // otherwise an interloper could inject decisions into the holder's turn.
    if let Err(response) =
        require_lease_holder(&state, &payload.session_id, payload.client_id.as_deref()).await
    {
        return response;
    }
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

    // Lease check: only the current holder may cancel an in-flight turn,
    // otherwise a non-holder could kill another client's turn mid-stream.
    if let Err(response) =
        require_lease_holder(&state, &session_id, payload.client_id.as_deref()).await
    {
        return response;
    }

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
    // Reject forged daemon-internal principals at the HTTP edge.
    if let Err(response) = reject_forged_daemon_principal(payload.client_id.as_deref()) {
        return response;
    }
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane
        .force_close_session_with_lease(&payload.session_id, payload.client_id.as_deref())
        .await
    {
        Ok(record) => {
            // Release the per-session diff tracker so its accumulated state
            // (per-call maps, file baselines) does not leak across the
            // daemon's lifetime.
            state.evict_diff_tracker(&payload.session_id);
            Json(record).into_response()
        }
        Err(error) => map_session_error(error),
    }
}

/// `POST /api/v1/runtimes/heartbeat` — renew the calling TUI's attach lease.
/// Returns 204 while the caller is still the holder; 409 with
/// `SessionAttachedByAnotherClient` body when another TUI has taken over,
/// signalling the original TUI to surface a takeover notice and stop
/// submitting.
async fn handle_session_heartbeat(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeHeartbeatRequest>,
) -> Response {
    // Reject forged daemon-internal principals at the HTTP edge.
    if let Err(response) = reject_forged_daemon_principal(payload.client_id.as_deref()) {
        return response;
    }
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.heartbeat_session(payload).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_session_error(error),
    }
}

/// `POST /api/v1/runtimes/detach` — release the calling TUI's attach lease
/// without destroying the session or its backend (used by exit / `/new` /
/// `/remote off` so the session stays warm for the next TUI). Idempotent:
/// returns 204 even when the caller never held the lease.
async fn handle_session_detach(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::gateway::RuntimeDetachRequest>,
) -> Response {
    // Reject forged daemon-internal principals at the HTTP edge.
    if let Err(response) = reject_forged_daemon_principal(payload.client_id.as_deref()) {
        return response;
    }
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };

    match control_plane.detach_session(payload).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_catalog(State(state): State<Arc<GatewayAppState>>) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };
    match control_plane.list_runtimes().await {
        Ok(runtimes) => Json(RuntimeCatalogResponse {
            runtimes: runtimes.into_iter().map(runtime_record_response).collect(),
        })
        .into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_cron_catalog(State(state): State<Arc<GatewayAppState>>) -> Response {
    let Some(scheduler) = state.cron_scheduler.as_ref() else {
        return Json(CronCatalogResponse {
            available: false,
            jobs: Vec::new(),
        })
        .into_response();
    };
    Json(CronCatalogResponse {
        available: true,
        jobs: scheduler.catalog().await,
    })
    .into_response()
}

async fn handle_channel_catalog(State(state): State<Arc<GatewayAppState>>) -> Response {
    Json(
        state
            .channel_manager
            .as_ref()
            .map(|manager| manager.catalog())
            .unwrap_or(ChannelCatalogResponse {
                channels: Vec::new(),
            }),
    )
    .into_response()
}

async fn handle_channel_test(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::daemon_protocol::wire::ChannelTestRequest>,
) -> Response {
    if payload.id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(GatewayErrorResponse {
                error: "channel id must not be empty".to_string(),
            }),
        )
            .into_response();
    }
    let Some(manager) = state.channel_manager.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GatewayErrorResponse {
                error: "no channels are loaded by the active daemon".to_string(),
            }),
        )
            .into_response();
    };
    match manager.test_connection(payload.id.trim()).await {
        Some(response) => Json(response).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(GatewayErrorResponse {
                error: format!("enabled channel '{}' was not found", payload.id.trim()),
            }),
        )
            .into_response(),
    }
}

async fn handle_cron_run(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::daemon_protocol::wire::CronRunRequest>,
) -> Response {
    let Some(scheduler) = state.cron_scheduler.as_ref() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(GatewayErrorResponse {
                error: "no enabled Cron jobs are loaded by the active daemon".to_string(),
            }),
        )
            .into_response();
    };
    if payload.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(GatewayErrorResponse {
                error: "Cron job name must not be empty".to_string(),
            }),
        )
            .into_response();
    }
    match scheduler.trigger_now(&payload.name).await {
        Ok(job) => (StatusCode::ACCEPTED, Json(CronRunResponse { job })).into_response(),
        Err(TriggerError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(GatewayErrorResponse {
                error: format!("enabled Cron job '{}' was not found", payload.name),
            }),
        )
            .into_response(),
        Err(TriggerError::AlreadyRunning) => (
            StatusCode::CONFLICT,
            Json(GatewayErrorResponse {
                error: format!("Cron job '{}' is already running", payload.name),
            }),
        )
            .into_response(),
    }
}

async fn handle_sandbox_catalog(State(state): State<Arc<GatewayAppState>>) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };
    match control_plane.list_sandboxes().await {
        Ok(sandboxes) => Json(SandboxCatalogResponse {
            sandboxes: sandboxes.into_iter().map(sandbox_catalog_item).collect(),
        })
        .into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_runtime_checkpoint_catalog(State(state): State<Arc<GatewayAppState>>) -> Response {
    let Some(control_plane) = state.session_control_plane.as_ref() else {
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(GatewayErrorResponse {
                error: "session control plane is not configured".to_string(),
            }),
        )
            .into_response();
    };
    match control_plane.list_runtime_checkpoints().await {
        Ok(checkpoints) => Json(RuntimeCheckpointCatalogResponse {
            checkpoints: checkpoints
                .into_iter()
                .map(runtime_checkpoint_catalog_item)
                .collect(),
        })
        .into_response(),
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
    // Lease guard: checkpoint mutates session state.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }

    match control_plane.checkpoint_runtime(payload).await {
        Ok(result) => Json(runtime_checkpoint_response(result)).into_response(),
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

    // No lease check: `checkout` creates a new child session from a checkpoint
    // — it doesn't mutate the source session's state, and the returned child
    // session_id can be lease-attached via `open_session`.
    match control_plane.checkout_runtime(payload).await {
        Ok(result) => Json(runtime_checkout_response(result)).into_response(),
        Err(error) => map_session_error(error),
    }
}

async fn handle_session_export(
    State(state): State<Arc<GatewayAppState>>,
    Json(payload): Json<xiaoo_shared::daemon_protocol::wire::RuntimeExportRequest>,
) -> Response {
    // Export returns the full SessionRecord (history, memory, agent state,
    // resolved LLM config). Restrict it to the current lease holder,
    // consistent with read_file/pause.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }
    match state
        .session_service
        .export_session(&payload.runtime_id)
        .await
    {
        Ok(mut session_data) => {
            // Drop the resolved LLM auth field from the export: the daemon
            // resolves it from the runtime store per request, so it is not
            // needed in the payload and should not be surfaced to callers.
            if let Some(llm) = session_data.runtime.llm.as_mut() {
                llm.api_key = None;
            }
            match serde_json::to_value(session_data) {
                Ok(content) => Json(RuntimeExportResponse {
                    runtime_id: payload.runtime_id.clone(),
                    format: RuntimeExportFormat::XiaooRuntimeSessionJson,
                    file_name: runtime_export_file_name(&payload.runtime_id),
                    content,
                })
                .into_response(),
                Err(error) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(GatewayErrorResponse {
                        error: format!("failed to serialize runtime export: {error}"),
                    }),
                )
                    .into_response(),
            }
        }
        Err(error) => map_session_error(error),
    }
}

fn runtime_export_file_name(runtime_id: &str) -> String {
    let safe_id: String = runtime_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let safe_id = if safe_id.is_empty() {
        "runtime"
    } else {
        &safe_id
    };
    format!("xiaoo-runtime-{safe_id}.json")
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
    // Lease guard: pause evicts the backend; a non-holder pausing would
    // evict the holder's sandbox out from under it.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }

    match control_plane.pause_runtime(payload).await {
        Ok(result) => Json(runtime_pause_response(result)).into_response(),
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
    // Lease guard: resume re-leases a backend — a non-holder could silently
    // reset the holder's paused session.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }

    match control_plane.resume_runtime(payload).await {
        Ok(result) => Json(runtime_resume_response(result)).into_response(),
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
    // No lease check: keyed by `checkpoint_id`, not `runtime_id` — admin
    // operation that deletes a remote provider snapshot (e.g. e2b).
    match control_plane.delete_checkpoint_snapshot(payload).await {
        Ok(result) => Json(runtime_checkpoint_snapshot_delete_response(result)).into_response(),
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
    // Lease guard: exec runs a shell in the sandbox.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }

    match control_plane.exec_runtime(payload).await {
        Ok(result) => Json(runtime_exec_response(result)).into_response(),
        Err(xiaoo_shared::gateway::SessionServiceError::RuntimeExecInterrupted {
            message,
            stdout_base64,
            stderr_base64,
            execution_state,
        }) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(RuntimeExecInterruptedResponse {
                error: format!("core runtime execution interrupted ({execution_state}): {message}"),
                execution_state: execution_state.to_string(),
                stdout_base64,
                stderr_base64,
                retryable: execution_state == xiaoo_api::backend::ExecutionState::NotStarted,
            }),
        )
            .into_response(),
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
    // Lease guard: read_file is read-only but gated to keep the policy
    // uniform ("all sandbox access requires the lease") and prevent snooping.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }

    match control_plane.read_runtime_file(payload).await {
        Ok(result) => Json(runtime_read_file_response(result)).into_response(),
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
    // Lease guard: write_file mutates files in the sandbox.
    if let Err(response) =
        require_lease_holder(&state, &payload.runtime_id, payload.client_id.as_deref()).await
    {
        return response;
    }

    match control_plane.write_runtime_file(payload).await {
        Ok(result) => Json(runtime_write_file_response(result)).into_response(),
        Err(error) => map_session_error(error),
    }
}

fn map_session_error(error: xiaoo_shared::gateway::SessionServiceError) -> Response {
    let status = match &error {
        xiaoo_shared::gateway::SessionServiceError::InvalidRequest { .. } => {
            StatusCode::BAD_REQUEST
        }
        xiaoo_shared::gateway::SessionServiceError::RuntimeConflict { .. } => StatusCode::CONFLICT,
        xiaoo_shared::gateway::SessionServiceError::PayloadTooLarge { .. } => {
            StatusCode::PAYLOAD_TOO_LARGE
        }
        xiaoo_shared::gateway::SessionServiceError::SessionNotFound { .. } => StatusCode::NOT_FOUND,
        xiaoo_shared::gateway::SessionServiceError::SessionBusy { .. } => {
            StatusCode::TOO_MANY_REQUESTS
        }
        xiaoo_shared::gateway::SessionServiceError::SessionClosed { .. } => StatusCode::CONFLICT,
        xiaoo_shared::gateway::SessionServiceError::SessionAttachedByAnotherClient { .. } => {
            StatusCode::CONFLICT
        }
        // Anonymous caller hit a route with `enforce_anonymous_lease = true`.
        // 401 (not 403) so the TUI can distinguish "missing client_id" from
        // "wrong client_id".
        xiaoo_shared::gateway::SessionServiceError::LeaseRequired { .. } => {
            StatusCode::UNAUTHORIZED
        }
        // Daemon wall clock is before UNIX_EPOCH — fail-closed with 503 so
        // the TUI retries (transient; NTP will correct it).
        xiaoo_shared::gateway::SessionServiceError::LeaseClockSkew { .. } => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        xiaoo_shared::gateway::SessionServiceError::UnsupportedCapability { .. } => {
            StatusCode::NOT_IMPLEMENTED
        }
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let body = session_error_body(&error);
    (status, Json(body)).into_response()
}

/// Build the JSON response body for a [`SessionServiceError`].
/// `SessionAttachedByAnotherClient` emits structured holder-identity fields
/// (so TUIs can read `holder_client_id` / `stale` without parsing a Display
/// string); other variants fall back to `{"error": "<Display>"}`.
fn session_error_body(error: &xiaoo_shared::gateway::SessionServiceError) -> serde_json::Value {
    match error {
        xiaoo_shared::gateway::SessionServiceError::SessionAttachedByAnotherClient {
            session_id,
            holder_client_id,
            holder_hostname,
            holder_pid,
            last_heartbeat_ms,
            stale,
        } => serde_json::json!({
            "error": "session is attached by another client; see structured fields for holder identity",
            "kind": "session_attached_by_another_client",
            "session_id": session_id,
            "holder_client_id": holder_client_id,
            "holder_hostname": holder_hostname,
            "holder_pid": holder_pid,
            "last_heartbeat_ms": last_heartbeat_ms,
            "stale": stale,
        }),
        _ => serde_json::json!({ "error": error.to_string() }),
    }
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
#[path = "../../../../tests/unit/serverside/httpserver/router_test.rs"]
mod tests;

fn filter_messages_for_display(
    messages: &[xiaoo_api::chat::ChatMessage],
) -> Vec<xiaoo_api::chat::ChatMessage> {
    messages.iter().map(filter_message_for_display).collect()
}

fn filter_message_for_display(
    message: &xiaoo_api::chat::ChatMessage,
) -> xiaoo_api::chat::ChatMessage {
    use xiaoo_api::chat::ContentBlock;
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

    xiaoo_api::chat::ChatMessage {
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
