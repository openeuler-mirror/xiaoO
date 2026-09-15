use super::*;
use super::{
    create_router_from_state, create_router_with_control_plane_and_auth, handle_channel_events,
    map_session_error, reject_forged_daemon_principal, runtime_export_file_name, GatewayAppState,
    GatewayErrorResponse, HttpBearerAuthConfig,
};
use crate::channel_management::ChannelManager;
use crate::channels::{
    AdapterResponse, ChannelAdapter, ChannelCapabilities, ChannelMember, ChannelMention,
    ChannelMessage, ChannelMeta, ChannelResult, ChannelRuntime, ChannelTextFormat,
};
use crate::cron::scheduler::CronScheduler;
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
use xiaoo_api::events::LoopEventSink;
use xiaoo_shared::backend::{BackendInfo, BackendLineageInfo};
use xiaoo_shared::cron::{CronExpression, CronJobConfig};
use xiaoo_shared::gateway::{
    AppTurnRequest, AppTurnResult, SessionControlPlane, SessionService, SessionServiceError,
    TurnOutcome,
};
use xiaoo_shared::{RuntimeExecRequest, RuntimeExecResult};

#[test]
fn runtime_export_file_name_is_safe_and_bounded() {
    assert_eq!(
        runtime_export_file_name("runtime/with spaces"),
        "xiaoo-runtime-runtime_with_spaces.json"
    );
    assert_eq!(runtime_export_file_name(""), "xiaoo-runtime-runtime.json");
    assert!(runtime_export_file_name(&"a".repeat(200)).len() <= 99);
}

struct InterruptedExecControlPlane;

#[async_trait]
impl SessionControlPlane for InterruptedExecControlPlane {
    async fn exec_runtime(
        &self,
        _request: RuntimeExecRequest,
    ) -> Result<RuntimeExecResult, SessionServiceError> {
        Err(SessionServiceError::RuntimeExecInterrupted {
            message: "stream reset".to_string(),
            stdout_base64: "cGFydGlhbA==".to_string(),
            stderr_base64: String::new(),
            execution_state: xiaoo_api::backend::ExecutionState::RunningOrCompleted,
        })
    }
}

struct SandboxCatalogControlPlane;

#[async_trait]
impl SessionControlPlane for SandboxCatalogControlPlane {
    async fn list_sandboxes(&self) -> Result<Vec<BackendInfo>, SessionServiceError> {
        let sandbox = serde_json::from_value(serde_json::json!({
            "backend_id": "sandbox-1",
            "provider": "e2b",
            "instance_id": "instance-private",
            "state": "active",
            "workspace_root": "/workspace",
            "endpoint": {"provider_handle": {"value": {"credential": "must-not-leak"}}},
            "metadata": {"api_key": "must-not-leak"},
            "resources": {"vcpu_count": 2, "memory_mb": 4096, "disk_mb": null},
            "session_id": "runtime-1",
            "session_ids": ["runtime-1"],
            "expires_at_ms": 1234,
            "lineage": BackendLineageInfo::default(),
        }))
        .expect("sandbox fixture should deserialize");
        Ok(vec![sandbox])
    }
}

#[tokio::test(flavor = "current_thread")]
async fn sandbox_catalog_is_authenticated_and_redacted() {
    let router = create_router_with_control_plane_and_auth(
        Arc::new(FakeSessionService::new("unused")),
        Arc::new(SandboxCatalogControlPlane),
        Some(HttpBearerAuthConfig::new("secret-token")),
        None,
        None,
        Arc::new(ChannelManager::new(None, None)),
    );

    let unauthorized = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/sandboxes")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

    let response = router
        .oneshot(
            Request::builder()
                .uri("/api/v1/sandboxes")
                .header("authorization", "Bearer secret-token")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let text = String::from_utf8(body.to_vec()).expect("response should be UTF-8");
    assert!(!text.contains("must-not-leak"));
    let payload: serde_json::Value = serde_json::from_str(&text).expect("response should be JSON");
    assert_eq!(payload["sandboxes"][0]["endpoint_kind"], "provider_handle");
    assert_eq!(payload["sandboxes"][0]["runtime_ids"][0], "runtime-1");
}

#[tokio::test(flavor = "current_thread")]
async fn runtime_exec_interruption_returns_partial_output() {
    let router = create_router_with_control_plane_and_auth(
        Arc::new(FakeSessionService::new("unused")),
        Arc::new(InterruptedExecControlPlane),
        None,
        None,
        None,
        Arc::new(ChannelManager::new(None, None)),
    );

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runtimes/exec")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"runtime_id":"runtime-1","command":"echo hello"}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value =
        serde_json::from_slice(&body).expect("response should be JSON");
    assert_eq!(payload["execution_state"], "running_or_completed");
    assert_eq!(payload["stdout_base64"], "cGFydGlhbA==");
    assert_eq!(payload["stderr_base64"], "");
    assert_eq!(payload["retryable"], false);
}

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
async fn runtime_export_uses_body_runtime_id_route() {
    let router = create_router_with_auth(
        Arc::new(FakeSessionService::new("unused")),
        Some(HttpBearerAuthConfig::new("secret-token")),
        None,
    );

    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runtimes/export")
                .header("authorization", "Bearer secret-token")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"runtime_id":"runtime-1"}"#))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

    let obsolete_response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/runtimes/export/runtime-1")
                .header("authorization", "Bearer secret-token")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("router should respond");
    assert_eq!(obsolete_response.status(), StatusCode::NOT_FOUND);
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
async fn bearer_auth_does_not_apply_to_metadata_or_feishu_webhook() {
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

    let capabilities_response = router
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/capabilities")
                .body(Body::empty())
                .expect("request should build"),
        )
        .await
        .expect("capabilities route should respond");
    assert_eq!(capabilities_response.status(), StatusCode::OK);
    let capabilities_body = to_bytes(capabilities_response.into_body(), usize::MAX)
        .await
        .expect("capabilities body should read");
    let capabilities: serde_json::Value =
        serde_json::from_slice(&capabilities_body).expect("capabilities response should be JSON");
    assert_eq!(capabilities["protocol_version"], 1);
    assert_eq!(capabilities["features"]["runtime_leases"], true);
    assert_eq!(capabilities["management"]["api_version"], 1);
    assert_eq!(
        capabilities["management"]["config_schema_version"],
        crate::config_schema::CONFIG_SCHEMA_VERSION
    );
    assert!(capabilities["management"]["domains"]
        .as_array()
        .expect("management domains")
        .iter()
        .any(|domain| domain["id"] == "models"
            && domain["actions"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|action| action == "session_select"))));

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
            cached_tokens: 0,
            total_tokens: 0,
            estimated_input_tokens: 0,
            outcome: TurnOutcome::Complete,
            hook_actions: Vec::new(),
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

#[tokio::test(flavor = "current_thread")]
async fn cron_runtime_routes_trigger_and_report_results() {
    let service = Arc::new(FakeSessionService::with_delay(
        "scheduled reply",
        Duration::from_millis(25),
    ));
    let scheduler = Arc::new(CronScheduler::new(
        vec![CronJobConfig {
            name: "daily-review".to_string(),
            description: None,
            cron: CronExpression::parse("0 0 1 1 *").expect("valid cron"),
            prompt: "Review the workspace".to_string(),
            agent_role: None,
            timeout_secs: 5,
            enabled: true,
            max_retries: 0,
            retry_delay_secs: 0,
        }],
        1,
        service,
    ));
    let mut state = GatewayAppState::new(Arc::new(FakeSessionService::new("unused")));
    state.set_cron_scheduler(Some(scheduler.clone()));
    let router = create_router_from_state(state, None, None);

    let catalog = router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/cron/jobs")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("catalog response");
    assert_eq!(catalog.status(), StatusCode::OK);
    let body = to_bytes(catalog.into_body(), usize::MAX)
        .await
        .expect("catalog body");
    let body: serde_json::Value = serde_json::from_slice(&body).expect("catalog JSON");
    assert_eq!(body["available"], true);
    assert_eq!(body["jobs"][0]["name"], "daily-review");

    let trigger_body = serde_json::json!({"name": "daily-review"}).to_string();
    let trigger = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cron/run")
                .header("content-type", "application/json")
                .body(Body::from(trigger_body.clone()))
                .expect("request"),
        )
        .await
        .expect("trigger response");
    assert_eq!(trigger.status(), StatusCode::ACCEPTED);

    let overlap = router
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/cron/run")
                .header("content-type", "application/json")
                .body(Body::from(trigger_body))
                .expect("request"),
        )
        .await
        .expect("overlap response");
    assert_eq!(overlap.status(), StatusCode::CONFLICT);

    timeout(Duration::from_secs(1), async {
        loop {
            let jobs = scheduler.catalog().await;
            if !jobs[0].running {
                assert_eq!(jobs[0].last_reply.as_deref(), Some("scheduled reply"));
                assert_eq!(jobs[0].success_count, 1);
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("Cron job should complete");
    scheduler.stop().await;
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

#[test]
fn bootstrap_errors_map_to_stable_http_statuses() {
    assert_eq!(
        map_session_error(SessionServiceError::InvalidRequest {
            message: "invalid".to_string(),
        })
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        map_session_error(SessionServiceError::RuntimeConflict {
            message: "conflict".to_string(),
        })
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        map_session_error(SessionServiceError::PayloadTooLarge {
            message: "large".to_string(),
        })
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

// ---------- Security: reject_forged_daemon_principal ----------

#[test]
fn reject_forged_daemon_principal_blocks_known_daemon_ids() {
    // Every daemon-internal principal must be rejected from HTTP.
    for forged in [
        xiaoo_shared::gateway::daemon_cron_principal(),
        xiaoo_shared::gateway::daemon_hook_principal("my-hook"),
        xiaoo_shared::gateway::daemon_channel_principal("feishu"),
        "daemon:attacker".to_string(),
        "daemon:".to_string(),
    ] {
        let result = reject_forged_daemon_principal(Some(&forged));
        assert!(
            result.is_err(),
            "HTTP client claiming `{forged}` must be rejected at the router edge"
        );
    }
}

#[test]
fn reject_forged_daemon_principal_passes_legitimate_client_ids() {
    assert!(reject_forged_daemon_principal(None).is_ok());
    assert!(reject_forged_daemon_principal(Some("")).is_ok());
    assert!(reject_forged_daemon_principal(Some("550e8400-e29b-41d4-a716-446655440000")).is_ok());
    // Check is prefix-based, not substring-based.
    assert!(
        reject_forged_daemon_principal(Some("user-daemon-test")).is_ok(),
        "non-prefix match must not be rejected"
    );
}

// ---------- HTTP lease guard: 409 contract ----------

/// `SessionControlPlane` stub whose `assert_lease_holder` always rejects
/// with `SessionAttachedByAnotherClient`, used to drive the 409 path.
struct LeaseRejectingControlPlane;

#[async_trait]
impl SessionControlPlane for LeaseRejectingControlPlane {
    async fn assert_lease_holder(
        &self,
        session_id: &str,
        _client_id: Option<&str>,
    ) -> Result<(), SessionServiceError> {
        Err(SessionServiceError::SessionAttachedByAnotherClient {
            session_id: session_id.to_string(),
            holder_client_id: "client-a".to_string(),
            holder_hostname: "holder-host".to_string(),
            holder_pid: 12345,
            last_heartbeat_ms: 67890,
            stale: false,
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn require_lease_holder_returns_409_with_structured_body() {
    // Drive the lease guard via /runtimes/exec. The router must surface
    // `SessionAttachedByAnotherClient` as HTTP 409 with the structured
    // JSON body the TUI parses.
    let router = create_router_with_control_plane_and_auth(
        Arc::new(FakeSessionService::new("unused")),
        Arc::new(LeaseRejectingControlPlane),
        None,
        None,
        None,
        Arc::new(ChannelManager::new(None, None)),
    );

    let response = router
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/runtimes/exec")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"runtime_id":"rt-1","command":"echo hi","client_id":"client-b"}"#,
                ))
                .expect("request should build"),
        )
        .await
        .expect("router should respond");

    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "SessionAttachedByAnotherClient must surface as 409 CONFLICT"
    );
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body should read");
    let payload: serde_json::Value =
        serde_json::from_slice(&body).expect("response should be JSON");
    // Contract markers the TUI parser requires.
    assert_eq!(
        payload["kind"], "session_attached_by_another_client",
        "TUI parser keys off `kind`; missing/wrong kind falls back to raw body"
    );
    assert_eq!(payload["holder_client_id"], "client-a");
    assert_eq!(payload["holder_hostname"], "holder-host");
    assert_eq!(payload["holder_pid"], 12345);
    assert_eq!(payload["last_heartbeat_ms"], 67890);
    assert_eq!(payload["stale"], false);
    // The `error` field must still be present for generic clients.
    assert!(
        payload["error"].is_string() && !payload["error"].as_str().unwrap().is_empty(),
        "generic clients still get a non-empty `error` summary"
    );
}

impl super::GatewayAppState {
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
            action_sink: None,
            cron_scheduler: None,
            channel_manager: None,
            session_diff_trackers: Arc::new(std::sync::RwLock::new(HashMap::new())),
        }
    }
}

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
