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
    let store: Arc<dyn SessionStore> = Arc::new(InMemorySessionStore::default());
    let resolver: Arc<dyn SessionRuntimeResolver> = Arc::new(NoopRuntimeResolver);
    let backend_manager = Arc::new(BackendManager::new());
    let session = SessionRecord {
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
    };
    let supervisor = SessionSupervisor::new(store, resolver, backend_manager, session, None);

    assert!(
        supervisor.current_root_sinks.lock().await.is_none(),
        "current_root_sinks should start as None"
    );
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
