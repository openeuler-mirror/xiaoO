//! Read-only dashboard surface for the xiaoo daemon.
//!
//! Exposes a small axum router served on a separate port from the main
//! runtime API so operators can inspect every session the daemon is
//! tracking, plus the sandboxes (operation backends) it owns and how the
//! two are linked. The router deliberately performs no mutation: it only
//! snapshots the in-memory session store and backend manager state.

use axum::{
    body::Body,
    extract::State,
    http::{header, HeaderValue, StatusCode, Uri},
    response::Response,
    routing::get,
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::Arc;
use xiaoo_shared::backend::{BackendInfo, BackendListFilter, BackendManager};
use xiaoo_shared::gateway::{SessionLifecycleStatus, SessionRecord, SessionStore};

#[derive(RustEmbed)]
#[folder = "static/"]
struct DashboardAssets;

/// Shared state for the dashboard router. Holds only the read-only handles
/// the operators need: the session store (for session listing) and the
/// backend manager (for sandbox listing). Both are the same `Arc`s the main
/// daemon router uses, so the dashboard always reflects live state.
#[derive(Clone)]
pub struct DashboardState {
    session_store: Arc<dyn SessionStore>,
    backend_manager: Arc<BackendManager>,
}

impl DashboardState {
    pub fn new(session_store: Arc<dyn SessionStore>, backend_manager: Arc<BackendManager>) -> Self {
        Self {
            session_store,
            backend_manager,
        }
    }
}

/// Build the dashboard router. Mount it on its own listener; it never
/// intersects the bearer-protected runtime API on the main port.
pub fn dashboard_router(state: DashboardState) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/index.html", get(index_handler))
        .route("/api/v1/dashboard/overview", get(handle_overview))
        .route("/api/v1/dashboard/sessions", get(handle_sessions))
        .route("/api/v1/dashboard/sandboxes", get(handle_sandboxes))
        .fallback(static_handler)
        .with_state(Arc::new(state))
}

async fn index_handler() -> Response {
    serve_asset("index.html")
}

/// Fallback handler that resolves embedded static assets (e.g. `app.js`,
/// `style.css`) by URL path. Mirrors the moirai CLI static asset pattern.
async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() || path == "index.html" {
        "index.html"
    } else {
        path
    };
    serve_asset(path)
}

fn serve_asset(path: &str) -> Response {
    match DashboardAssets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            Response::builder()
                .status(StatusCode::OK)
                .header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_str(mime.as_ref())
                        .expect("mime_guess output is always valid ASCII"),
                )
                .body(Body::from(content.data))
                .expect("fresh Response builder with valid content-type")
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Body::from("404 Not Found"))
            .expect("fresh Response builder with valid content-type"),
    }
}

#[derive(Debug, Serialize)]
struct DashboardOverview {
    sessions: SessionSummary,
    sandboxes: SandboxSummary,
    /// Sessions whose `backend_instance` is `None` while status is `Running`
    /// or `Idle` — i.e. they currently have no live sandbox attached.
    sessions_without_sandbox: usize,
    /// Sandboxes not currently leased by any session.
    orphan_sandboxes: usize,
    server_time_ms: u64,
}

#[derive(Debug, Serialize)]
struct SessionSummary {
    total: usize,
    by_status: BTreeMap<String, usize>,
}

#[derive(Debug, Serialize)]
struct SandboxSummary {
    total: usize,
    by_provider: BTreeMap<String, usize>,
    by_state: BTreeMap<String, usize>,
}

async fn handle_overview(State(state): State<Arc<DashboardState>>) -> Json<DashboardOverview> {
    let sessions = state.session_store.list_all().await;
    let sandboxes = state
        .backend_manager
        .list_backends(BackendListFilter::default())
        .await;

    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    for s in &sessions {
        *by_status
            .entry(session_status_label(s.status.clone()))
            .or_default() += 1;
    }

    let mut by_provider: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_state: BTreeMap<String, usize> = BTreeMap::new();
    for b in &sandboxes {
        *by_provider.entry(b.provider.clone()).or_default() += 1;
        *by_state
            .entry(backend_state_label(b.state).to_string())
            .or_default() += 1;
    }

    let sessions_without_sandbox = sessions
        .iter()
        .filter(|s| {
            s.backend_instance.is_none()
                && matches!(
                    s.status,
                    SessionLifecycleStatus::Idle | SessionLifecycleStatus::Running
                )
        })
        .count();
    let orphan_sandboxes = sandboxes
        .iter()
        .filter(|b| b.session_ids.is_empty())
        .count();

    Json(DashboardOverview {
        sessions: SessionSummary {
            total: sessions.len(),
            by_status,
        },
        sandboxes: SandboxSummary {
            total: sandboxes.len(),
            by_provider,
            by_state,
        },
        sessions_without_sandbox,
        orphan_sandboxes,
        server_time_ms: current_time_ms(),
    })
}

#[derive(Debug, Serialize)]
struct SessionCardDto {
    session_id: String,
    conversation_id: String,
    sender_id: String,
    channel: Option<String>,
    channel_instance_id: Option<String>,
    status: String,
    agent_id: String,
    model: String,
    backend_id: Option<String>,
    backend_state: Option<String>,
    backend_provider: Option<String>,
    backend_instance_id: Option<String>,
    backend_endpoint: Option<String>,
    parent_runtime_id: Option<String>,
    forked_from_checkpoint_id: Option<String>,
    last_error: Option<String>,
    created_at_ms: u64,
    updated_at_ms: u64,
}

async fn handle_sessions(State(state): State<Arc<DashboardState>>) -> Json<Vec<SessionCardDto>> {
    let records = state.session_store.list_all().await;
    let dtos = records.into_iter().map(map_session_to_card).collect();
    Json(dtos)
}

fn map_session_to_card(record: SessionRecord) -> SessionCardDto {
    let backend = record.backend_instance.as_ref();
    SessionCardDto {
        session_id: record.session_id,
        conversation_id: record.conversation_id,
        sender_id: record.sender_id,
        channel: record.channel,
        channel_instance_id: record.channel_instance_id,
        status: session_status_label(record.status),
        agent_id: record.runtime.agent_id.0,
        model: record.runtime.model,
        backend_id: backend.map(|b| b.backend_id.0.clone()),
        backend_state: backend.map(|b| backend_state_label(b.state).to_string()),
        backend_provider: backend.map(|b| b.provider.0.clone()),
        backend_instance_id: backend.map(|b| b.instance_id.0.clone()),
        backend_endpoint: backend.and_then(|b| backend_endpoint_str(b.endpoint.clone())),
        parent_runtime_id: record.parent_runtime_id,
        forked_from_checkpoint_id: record.forked_from_checkpoint_id,
        last_error: record.last_error,
        created_at_ms: record.created_at_ms,
        updated_at_ms: record.updated_at_ms,
    }
}

async fn handle_sandboxes(State(state): State<Arc<DashboardState>>) -> Json<Vec<BackendInfo>> {
    let sandboxes = state
        .backend_manager
        .list_backends(BackendListFilter::default())
        .await;
    Json(sandboxes)
}

fn session_status_label(status: SessionLifecycleStatus) -> String {
    match status {
        SessionLifecycleStatus::Idle => "idle".to_string(),
        SessionLifecycleStatus::Running => "running".to_string(),
        SessionLifecycleStatus::Paused => "paused".to_string(),
        SessionLifecycleStatus::Failed => "failed".to_string(),
        SessionLifecycleStatus::Closed => "closed".to_string(),
    }
}

fn backend_state_label(state: agent_contracts::backend::BackendLifecycleState) -> &'static str {
    use agent_contracts::backend::BackendLifecycleState;
    match state {
        BackendLifecycleState::Unknown => "unknown",
        BackendLifecycleState::Creating => "creating",
        BackendLifecycleState::Active => "active",
        BackendLifecycleState::Pausing => "pausing",
        BackendLifecycleState::Paused => "paused",
        BackendLifecycleState::Loading => "loading",
        BackendLifecycleState::Deleting => "deleting",
        BackendLifecycleState::Deleted => "deleted",
        BackendLifecycleState::Failed => "failed",
    }
}

fn backend_endpoint_str(
    endpoint: Option<agent_contracts::backend::BackendEndpoint>,
) -> Option<String> {
    use agent_contracts::backend::BackendEndpoint;
    endpoint.map(|e| match e {
        BackendEndpoint::Local => "local".to_string(),
        BackendEndpoint::Tcp { host, port } => format!("tcp://{host}:{port}"),
        BackendEndpoint::UnixSocket { path } => format!("unix:{path}"),
        BackendEndpoint::ProviderHandle { value } => {
            format!("provider:{}", value)
        }
    })
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_contracts::backend::BackendLifecycleState;
    use agent_types::common::ids::AgentId;
    use agent_types::context::{FeatureFlags, TokenBudgetConfig};
    use std::path::PathBuf;
    use std::sync::Arc;
    use xiaoo_shared::gateway::{InMemorySessionStore, SessionLifecycleStatus};

    fn fake_record(session_id: &str, status: SessionLifecycleStatus) -> SessionRecord {
        SessionRecord {
            session_id: session_id.to_string(),
            conversation_id: format!("conv-{session_id}"),
            sender_id: "tester".to_string(),
            entry: Default::default(),
            channel: Some("http".to_string()),
            channel_instance_id: None,
            status,
            runtime: xiaoo_shared::gateway::SessionRuntimeSnapshot {
                agent_id: AgentId("main".to_string()),
                model: "test-model".to_string(),
                llm: None,
                system_prompt: String::new(),
                feature_flags: FeatureFlags::default(),
                token_budget: TokenBudgetConfig {
                    total_budget: 0,
                    reserved_for_output: 0,
                    reserved_for_system: 0,
                    hard_limit_ratio: 0.0,
                },
                workspace_root: PathBuf::from("/tmp"),
                max_turns: None,
                tool_manifest: None,
                subagent_roles: Default::default(),
                bootstrap_binding: None,
            },
            backend_instance: None,
            paused_backend_checkpoint: None,
            loop_state: None,
            memory_snapshot: None,
            agents: Default::default(),
            subagent_state: Default::default(),
            last_error: None,
            parent_runtime_id: None,
            forked_from_checkpoint_id: None,
            created_at_ms: 1,
            updated_at_ms: 2,
        }
    }

    fn tmp_storage_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "xiaoo-dashboard-test-{}-{label}-{}",
            std::process::id(),
            current_time_ms()
        ))
    }

    #[tokio::test]
    async fn overview_counts_sessions_by_status() {
        let store = InMemorySessionStore::default();
        store
            .save(fake_record("s1", SessionLifecycleStatus::Running))
            .await;
        store
            .save(fake_record("s2", SessionLifecycleStatus::Running))
            .await;
        store
            .save(fake_record("s3", SessionLifecycleStatus::Paused))
            .await;

        let state = DashboardState::new(
            Arc::new(store) as Arc<dyn SessionStore>,
            Arc::new(BackendManager::new_with_storage_dir(
                Default::default(),
                tmp_storage_dir("overview"),
            )),
        );

        let Json(overview) = handle_overview(State(Arc::new(state))).await;
        assert_eq!(overview.sessions.total, 3);
        assert_eq!(overview.sessions.by_status.get("running"), Some(&2));
        assert_eq!(overview.sessions.by_status.get("paused"), Some(&1));
        assert_eq!(overview.sessions_without_sandbox, 2);
    }

    #[tokio::test]
    async fn sessions_endpoint_returns_cards_in_updated_order() {
        let store = InMemorySessionStore::default();
        let mut older = fake_record("older", SessionLifecycleStatus::Idle);
        older.updated_at_ms = 10;
        let mut newer = fake_record("newer", SessionLifecycleStatus::Running);
        newer.updated_at_ms = 100;
        store.save(older).await;
        store.save(newer).await;

        let state = DashboardState::new(
            Arc::new(store) as Arc<dyn SessionStore>,
            Arc::new(BackendManager::new_with_storage_dir(
                Default::default(),
                tmp_storage_dir("sessions"),
            )),
        );

        let Json(cards) = handle_sessions(State(Arc::new(state))).await;
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].session_id, "newer");
        assert_eq!(cards[0].status, "running");
        assert_eq!(cards[0].agent_id, "main");
        assert_eq!(cards[0].model, "test-model");
    }

    #[tokio::test]
    async fn sandboxes_endpoint_returns_empty_vec_for_fresh_manager() {
        let state = DashboardState::new(
            Arc::new(InMemorySessionStore::default()) as Arc<dyn SessionStore>,
            Arc::new(BackendManager::new_with_storage_dir(
                Default::default(),
                tmp_storage_dir("sandboxes"),
            )),
        );

        let Json(sandboxes) = handle_sandboxes(State(Arc::new(state))).await;
        assert!(sandboxes.is_empty());
    }

    #[test]
    fn backend_state_label_covers_all_variants() {
        let all = [
            BackendLifecycleState::Unknown,
            BackendLifecycleState::Creating,
            BackendLifecycleState::Active,
            BackendLifecycleState::Pausing,
            BackendLifecycleState::Paused,
            BackendLifecycleState::Loading,
            BackendLifecycleState::Deleting,
            BackendLifecycleState::Deleted,
            BackendLifecycleState::Failed,
        ];
        for state in all {
            assert!(!backend_state_label(state).is_empty());
        }
    }
}
