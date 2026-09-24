use super::*;
use crate::gateway::{InMemorySessionStore, SessionRuntimeBuildInput, SessionRuntimeResolveError};
use std::collections::BTreeMap;

#[test]
fn profile_reasoning_is_default_and_explicit_turn_value_wins() {
    let runtime_llm = crate::gateway::LlmRuntimeConfig {
        reasoning_effort: Some(ReasoningEffort::High),
        ..Default::default()
    };

    assert_eq!(
        effective_reasoning_effort(None, Some(&runtime_llm)),
        ReasoningEffort::High
    );
    assert_eq!(
        effective_reasoning_effort(Some(ReasoningEffort::Off), Some(&runtime_llm)),
        ReasoningEffort::Off
    );
}

/// Pins that `SessionSupervisor::new` initializes both sink fields
/// to `None`, so `run_lane_until_terminal`'s `is_none()` guard
/// correctly skips injection when no root turn is in progress.
#[tokio::test]
async fn new_initializes_sink_inheritance_fields_to_none() {
    let supervisor = test_supervisor().await;

    assert!(
        supervisor.current_root_sinks.lock().await.is_none(),
        "current_root_sinks should start as None"
    );
}

/// The close token starts live and `force_close` cancels it.
#[tokio::test]
async fn force_close_cancels_the_session_close_token() {
    let supervisor = test_supervisor().await;

    assert!(
        !supervisor.close_token.is_cancelled(),
        "the close token must start live"
    );

    let _ = supervisor.force_close().await;

    assert!(
        supervisor.close_token.is_cancelled(),
        "force_close must cancel the close token"
    );
}

/// A lane spawned during a turn runs on a child of the close token, so
/// `force_close` cancels it through parent->child propagation.
#[tokio::test]
async fn force_close_cancels_a_composed_lane_token() {
    let supervisor = test_supervisor().await;
    let (lane_token, _watcher) =
        compose_lane_token(&supervisor.close_token, Some(CancellationToken::new()));

    let _ = supervisor.force_close().await;

    tokio::time::timeout(Duration::from_secs(1), lane_token.cancelled())
        .await
        .expect("force_close must cancel the lane token");
}

/// A turn-level cancel stops just that lane and never reaches the
/// session's close token.
#[tokio::test]
async fn turn_cancel_stops_only_the_lane_token() {
    let close_token = CancellationToken::new();
    let turn_token = CancellationToken::new();
    let (lane_token, _watcher) = compose_lane_token(&close_token, Some(turn_token.clone()));

    turn_token.cancel();

    tokio::time::timeout(Duration::from_secs(1), lane_token.cancelled())
        .await
        .expect("turn cancel must fire the lane token");
    assert!(!close_token.is_cancelled());
}

/// The watcher is aborted when the lane exits, so a later turn cancel
/// cannot fire the retired lane token.
#[tokio::test]
async fn aborted_watcher_no_longer_links_the_turn_token() {
    let close_token = CancellationToken::new();
    let turn_token = CancellationToken::new();
    let (lane_token, watcher) = compose_lane_token(&close_token, Some(turn_token.clone()));
    let watcher = watcher.expect("a turn token must spawn a watcher");
    watcher.abort();
    let _ = watcher.await;

    turn_token.cancel();

    assert!(!lane_token.is_cancelled());
}

/// Lanes spawned outside a turn run on the close token itself.
#[tokio::test]
async fn lane_without_turn_token_runs_on_the_close_token() {
    let close_token = CancellationToken::new();
    let (lane_token, watcher) = compose_lane_token(&close_token, None);

    assert!(watcher.is_none());
    close_token.cancel();
    assert!(lane_token.is_cancelled());
}

/// Hibernating (pausing) a session is not closing it: the close token
/// stays live so hibernated lanes are not cancelled by an idle timeout.
#[tokio::test]
async fn hibernate_idle_does_not_cancel_the_close_token() {
    let supervisor = test_supervisor().await;

    let _ = supervisor.hibernate_idle().await;

    assert!(
        !supervisor.close_token.is_cancelled(),
        "hibernate_idle must not cancel the close token"
    );
}

async fn test_supervisor() -> Arc<SessionSupervisor> {
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let resolver: Arc<dyn SessionRuntimeResolver> = Arc::new(NoopRuntimeResolver);
    let backend_manager = Arc::new(BackendManager::new());
    let session = test_session_record();
    Arc::new(SessionSupervisor::new(
        store,
        resolver,
        backend_manager,
        session,
        None,
    ))
}

fn test_session_record() -> SessionRecord {
    SessionRecord {
        session_id: "test-session".to_string(),
        conversation_id: "test-session".to_string(),
        sender_id: "test-user".to_string(),
        entry: crate::gateway::GatewayEntryContext::tui(None),
        channel: None,
        channel_instance_id: None,
        status: SessionLifecycleStatus::Idle,
        runtime: crate::gateway::session_record::SessionRuntimeSnapshot {
            agent_id: AgentId("root-agent".to_string()),
            model: "stub-model".to_string(),
            llm: None,
            system_prompt: String::new(),
            feature_flags: agent_types::context::FeatureFlags::default(),
            token_budget: agent_types::context::TokenBudgetConfig {
                total_budget: 4096,
                reserved_for_output: 1024,
                reserved_for_system: 256,
                hard_limit_ratio: 0.9,
            },
            workspace_root: std::path::PathBuf::from("/tmp"),
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
        created_at_ms: 0,
        updated_at_ms: 0,
    }
}

/// Stub resolver for tests that do not exercise `resolve()`.
struct NoopRuntimeResolver;

#[async_trait::async_trait]
impl SessionRuntimeResolver for NoopRuntimeResolver {
    async fn resolve(
        &self,
        _request: &SessionRuntimeBuildInput,
        _existing: Option<&SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        Err(SessionRuntimeResolveError::ResolveFailed {
            message: "NoopRuntimeResolver does not resolve".to_string(),
        })
    }
}
