use super::*;
use async_trait::async_trait;
use axum::body::{to_bytes, Body};
use axum::http::Request as HttpRequest;
use std::collections::BTreeMap;
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tower::ServiceExt;
use xiaoo_api::chat::{FeatureFlags, TokenBudgetConfig};
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
        cached_tokens: 0,
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
        _interaction_handle: Option<Arc<dyn xiaoo_api::interaction::InteractionHandle>>,
        _channel_file_sender: Option<Arc<dyn xiaoo_shared::channels::ChannelFileSender>>,
        _cancellation_token: Option<CancellationToken>,
        _tool_event_sink: Option<Arc<dyn xiaoo_api::events::ToolEventSink>>,
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
    assert!(canonicalize_agent_workspace(Path::new("/definitely/not/a/xiaoo/workspace")).is_err());

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
    let second = serde_json::to_value(operations.status(&operation_id).expect("updated status"))
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

    let started_permit = tokio::time::timeout(std::time::Duration::from_secs(1), started.acquire())
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

impl super::AgentOperationRegistry {
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
}
