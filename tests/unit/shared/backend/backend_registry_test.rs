use super::*;
use tempfile::TempDir;

/// Build a registry backed by a fresh temporary directory so the test
/// never touches the shared `~/.xiaoo/backend_registry.json` used by a
/// running daemon. The returned `TempDir` must outlive the registry.
fn temp_registry() -> (TempDir, BackendRegistry) {
    let dir = TempDir::new().expect("tempdir");
    let registry = BackendRegistry::new_with_storage_dir(dir.path().to_path_buf());
    (dir, registry)
}

#[tokio::test]
async fn test_register_and_unregister() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-register");
    let entry = BackendRegistryEntry::new(
        "backend-1".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-1".to_string(),
        "e2b-sandbox-1".to_string(),
        None,
    );

    registry.register(entry).await.expect("should register");

    let retrieved = registry
        .get_entry("backend-1")
        .await
        .expect("should get entry");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().backend_id, "backend-1");

    registry
        .unregister("backend-1")
        .await
        .expect("should unregister");

    let retrieved = registry
        .get_entry("backend-1")
        .await
        .expect("should get entry");
    assert!(retrieved.is_none());
}

#[tokio::test]
async fn test_get_entries_by_key() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-key-multi-v2");

    for i in 0..3 {
        let entry = BackendRegistryEntry::new(
            format!("backend-mv2-{}", i),
            key.clone(),
            vec![format!("session-mv2-{}", i)],
            format!("process-mv2-{}", i),
            format!("e2b-sandbox-mv2-{}", i),
            None,
        );
        registry.register(entry).await.expect("should register");
    }

    let entries = registry
        .get_entries_by_key(&key)
        .await
        .expect("should get entries");
    assert_eq!(entries.len(), 3);
}

#[tokio::test]
async fn test_pending_eviction() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-eviction");
    let entry = BackendRegistryEntry::new(
        "backend-eviction-v2".to_string(),
        key,
        vec!["session-eviction".to_string()],
        "process-eviction".to_string(),
        "e2b-sandbox-eviction".to_string(),
        None,
    );

    registry.register(entry).await.expect("should register");

    registry
        .set_pending_eviction("backend-eviction-v2", true)
        .await
        .expect("should set pending");

    let retrieved = registry
        .get_entry("backend-eviction-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");
    assert!(retrieved.pending_eviction);

    registry
        .set_pending_eviction("backend-eviction-v2", false)
        .await
        .expect("should clear pending");

    let retrieved = registry
        .get_entry("backend-eviction-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");
    assert!(!retrieved.pending_eviction);
}

#[tokio::test]
async fn test_update_activity() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-activity");
    let entry = BackendRegistryEntry::new(
        "backend-activity-v2".to_string(),
        key,
        vec!["session-activity".to_string()],
        "process-activity".to_string(),
        "e2b-sandbox-activity".to_string(),
        None,
    );

    registry
        .register(entry.clone())
        .await
        .expect("should register");

    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    registry
        .update_activity("backend-activity-v2")
        .await
        .expect("should update activity");

    let retrieved = registry
        .get_entry("backend-activity-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");
    assert!(retrieved.last_activity_ms > entry.last_activity_ms);
}

#[tokio::test]
async fn test_get_entries_for_process() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-process-v2");

    let entry1 = BackendRegistryEntry::new(
        "backend-pv2-1".to_string(),
        key.clone(),
        vec!["session-p1".to_string()],
        "process-A".to_string(),
        "e2b-sandbox-p1".to_string(),
        None,
    );
    let entry2 = BackendRegistryEntry::new(
        "backend-pv2-2".to_string(),
        key,
        vec!["session-p2".to_string()],
        "process-A".to_string(),
        "e2b-sandbox-p2".to_string(),
        None,
    );
    let entry3 = BackendRegistryEntry::new(
        "backend-pv2-3".to_string(),
        SandboxCounterKey::new("e2b", "other-key"),
        vec!["session-p3".to_string()],
        "process-B".to_string(),
        "e2b-sandbox-p3".to_string(),
        None,
    );

    registry.register(entry1).await.expect("should register");
    registry.register(entry2).await.expect("should register");
    registry.register(entry3).await.expect("should register");

    let entries_a = registry
        .get_entries_for_process("process-A")
        .await
        .expect("should get entries");
    assert_eq!(entries_a.len(), 2);

    let entries_b = registry
        .get_entries_for_process("process-B")
        .await
        .expect("should get entries");
    assert_eq!(entries_b.len(), 1);
}

#[tokio::test]
async fn test_update_session_status() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-status-v2");
    let entry = BackendRegistryEntry::new(
        "backend-status-v2".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-1".to_string(),
        "e2b-sandbox-status".to_string(),
        None,
    );

    registry.register(entry).await.expect("should register");

    registry
        .update_session_status("backend-status-v2", "session-1", "running", 1)
        .await
        .expect("should update status");

    let retrieved = registry
        .get_entry("backend-status-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");

    let status = retrieved
        .get_session_status("session-1")
        .expect("should have status");
    assert_eq!(status.status, "running");
    assert_eq!(status.queue_depth, 1);

    registry
        .update_session_status("backend-status-v2", "session-1", "idle", 0)
        .await
        .expect("should update status");

    let retrieved = registry
        .get_entry("backend-status-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");

    assert!(retrieved.is_all_sessions_idle());
}

#[tokio::test]
async fn test_is_all_sessions_idle() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-idle-v2");
    let entry = BackendRegistryEntry::new(
        "backend-idle-v2".to_string(),
        key,
        vec!["session-1".to_string(), "session-2".to_string()],
        "process-1".to_string(),
        "e2b-sandbox-idle".to_string(),
        None,
    );

    registry.register(entry).await.expect("should register");

    registry
        .update_session_status("backend-idle-v2", "session-1", "idle", 0)
        .await
        .expect("should update status");
    registry
        .update_session_status("backend-idle-v2", "session-2", "idle", 0)
        .await
        .expect("should update status");

    let retrieved = registry
        .get_entry("backend-idle-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");
    assert!(retrieved.is_all_sessions_idle());

    registry
        .update_session_status("backend-idle-v2", "session-1", "running", 1)
        .await
        .expect("should update status");

    let retrieved = registry
        .get_entry("backend-idle-v2")
        .await
        .expect("should get entry")
        .expect("should have entry");
    assert!(!retrieved.is_all_sessions_idle());
}

#[tokio::test]
async fn test_refresh_heartbeats_for_process() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-heartbeat");

    let entry_a = BackendRegistryEntry::new(
        "backend-hb-a".to_string(),
        key.clone(),
        vec!["session-a".to_string()],
        "process-A".to_string(),
        "e2b-hb-a".to_string(),
        None,
    );
    let entry_b = BackendRegistryEntry::new(
        "backend-hb-b".to_string(),
        key,
        vec!["session-b".to_string()],
        "process-B".to_string(),
        "e2b-hb-b".to_string(),
        None,
    );
    registry.register(entry_a).await.expect("register a");
    registry.register(entry_b).await.expect("register b");

    // Capture B's heartbeat, then refresh A's.
    let before_b = registry
        .get_entry("backend-hb-b")
        .await
        .expect("get b")
        .expect("has b")
        .owner_heartbeat_ms;
    tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
    registry
        .refresh_heartbeats_for_process("process-A")
        .await
        .expect("refresh");

    let a = registry
        .get_entry("backend-hb-a")
        .await
        .expect("get a")
        .expect("has a");
    let b = registry
        .get_entry("backend-hb-b")
        .await
        .expect("get b")
        .expect("has b");
    // A was refreshed, B was not touched.
    assert!(a.owner_heartbeat_ms >= b.owner_heartbeat_ms);
    assert_eq!(b.owner_heartbeat_ms, before_b);
}

#[tokio::test]
async fn test_is_evictable_when_owner_stale() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-stale-owner");
    let mut entry = BackendRegistryEntry::new(
        "backend-stale".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-dead".to_string(),
        "e2b-sandbox-stale".to_string(),
        None,
    );
    // Session is running (not idle), but owner heartbeat is very old.
    entry.update_session_status("session-1", "running", 1);
    entry.owner_heartbeat_ms = current_time_ms().saturating_sub(60_000);
    registry.register(entry).await.expect("register");

    let retrieved = registry
        .get_entry("backend-stale")
        .await
        .expect("get")
        .expect("has entry");
    // Running session → not idle, but stale owner → evictable.
    assert!(!retrieved.is_all_sessions_idle());
    assert!(retrieved.is_owner_stale(30_000));
    assert!(retrieved.is_evictable(30_000));
}

#[tokio::test]
async fn test_is_evictable_when_idle_and_owner_alive() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-alive-owner");
    let mut entry = BackendRegistryEntry::new(
        "backend-alive".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-alive".to_string(),
        "e2b-sandbox-alive".to_string(),
        None,
    );
    entry.update_session_status("session-1", "idle", 0);
    registry.register(entry).await.expect("register");

    let retrieved = registry
        .get_entry("backend-alive")
        .await
        .expect("get")
        .expect("has entry");
    assert!(retrieved.is_all_sessions_idle());
    assert!(!retrieved.is_owner_stale(30_000));
    assert!(retrieved.is_evictable(30_000));
}

#[tokio::test]
async fn test_not_evictable_when_running_and_owner_alive() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-busy-owner");
    let mut entry = BackendRegistryEntry::new(
        "backend-busy".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-alive".to_string(),
        "e2b-sandbox-busy".to_string(),
        None,
    );
    entry.update_session_status("session-1", "running", 1);
    registry.register(entry).await.expect("register");

    let retrieved = registry
        .get_entry("backend-busy")
        .await
        .expect("get")
        .expect("has entry");
    assert!(!retrieved.is_all_sessions_idle());
    assert!(!retrieved.is_owner_stale(30_000));
    assert!(!retrieved.is_evictable(30_000));
}

/// Regression test for the root-turn race (apps/shared/src/gateway/
/// session_supervisor.rs::run_root_turn):
///
/// When `ensure_session_backend` creates a new backend for a session
/// that is about to run a turn, the caller passes
/// `initial_session_status: Some(("running", 1))` so the freshly
/// registered backend is not eligible for eviction while the turn is
/// in flight. Before this fix, `BackendRegistryEntry::new` always
/// initialised `session_statuses` to the "idle" default, and another
/// process under sandbox pressure could evict the brand-new sandbox
/// mid-turn.
#[tokio::test]
async fn test_new_with_initial_running_status_is_not_evictable() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-initial-running");
    let entry = BackendRegistryEntry::new(
        "backend-initial-running".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-alive".to_string(),
        "e2b-sandbox-initial-running".to_string(),
        Some(("running", 1)),
    );
    registry.register(entry).await.expect("register");

    let retrieved = registry
        .get_entry("backend-initial-running")
        .await
        .expect("get")
        .expect("has entry");
    // Constructed with status="running", queue_depth=1 → not idle and
    // not evictable while the owner is alive. This is the property that
    // closes the root-turn eviction race.
    assert!(!retrieved.is_all_sessions_idle());
    assert!(!retrieved.is_owner_stale(30_000));
    assert!(!retrieved.is_evictable(30_000));

    let status = retrieved
        .get_session_status("session-1")
        .expect("session-1 status");
    assert_eq!(status.status, "running");
    assert_eq!(status.queue_depth, 1);
}

/// Regression test for the cross-process eviction bug.
///
/// When a backend is marked `pending_eviction=true` (by another process
/// requesting eviction via SIGUSR1), `is_evictable` returns false (it
/// excludes already-pending entries so they aren't selected twice).
/// The owner process's `check_and_evict_marked_backends` handler must
/// still be able to confirm the backend is safe to delete via
/// `is_eviction_safe` — otherwise the handler would clear the mark and
/// skip every time, never actually evicting, and the requesting process
/// would fail with "sandbox limit reached" even though idle sandboxes
/// are available to evict.
#[tokio::test]
async fn test_is_eviction_safe_ignores_pending_eviction_for_marked_entry() {
    let (_dir, registry) = temp_registry();
    let key = SandboxCounterKey::new("e2b", "test-marked-eviction");
    let mut entry = BackendRegistryEntry::new(
        "backend-marked".to_string(),
        key,
        vec!["session-1".to_string()],
        "process-alive".to_string(),
        "e2b-sandbox-marked".to_string(),
        None,
    );
    // Session is idle and owner is alive — the normal "evict me" case.
    entry.update_session_status("session-1", "idle", 0);
    // Another process has marked this backend for eviction.
    entry.pending_eviction = true;
    registry.register(entry).await.expect("register");

    let retrieved = registry
        .get_entry("backend-marked")
        .await
        .expect("get")
        .expect("has entry");

    // `is_evictable` returns false because pending_eviction is true —
    // this is what `try_evict_if_needed` uses to skip already-marked
    // entries when selecting a victim.
    assert!(!retrieved.is_evictable(30_000));
    // But `is_eviction_safe` must return true so the owner's signal
    // handler actually performs the eviction instead of clearing the
    // mark and skipping.
    assert!(retrieved.is_eviction_safe(30_000));
}

impl super::BackendRegistryEntry {
    pub(crate) fn get_session_status(&self, session_id: &str) -> Option<&SessionStatusSnapshot> {
        self.session_statuses.get(session_id)
    }
}

impl super::BackendRegistry {
    pub(crate) async fn get_entry(
        &self,
        backend_id: &str,
    ) -> Result<Option<BackendRegistryEntry>, BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;

        Ok(data.entries.get(backend_id).cloned())
    }
}
