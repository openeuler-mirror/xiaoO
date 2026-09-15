use super::*;
use std::path::PathBuf;
use std::sync::Arc;
use xiaoo_api::chat::{AgentId, FeatureFlags, TokenBudgetConfig};
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
