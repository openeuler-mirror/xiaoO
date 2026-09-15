use super::*;
use agent_contracts::backend::{
    BackendLifecycleReason, BackendPath, BackendPauseMode, BackendResourceLimits, BackendSnapshotId,
};

#[test]
fn local_delete_is_noop_success() {
    let provider = local_backend_provider();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");

    let outcome = runtime
        .block_on(provider.delete(BackendDeleteRequest {
            backend_id: BackendId("local:test".to_string()),
            instance_id: Some(BackendInstanceId("local:test".to_string())),
            snapshot_id: None,
            force: false,
            reason: BackendLifecycleReason::SessionClose,
            metadata: Value::Null,
        }))
        .expect("delete should succeed");

    assert_eq!(outcome.state, BackendLifecycleState::Deleted);
    assert!(!outcome.already_deleted);
}

#[test]
fn local_pause_is_unsupported() {
    let provider = local_backend_provider();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");

    let result = runtime.block_on(provider.pause(BackendPauseRequest {
        backend_id: BackendId("local:test".to_string()),
        instance_id: BackendInstanceId("local:test".to_string()),
        mode: BackendPauseMode::BestEffort,
        reason: BackendLifecycleReason::SessionIdle,
        metadata: Value::Null,
    }));

    assert!(matches!(
        result,
        Err(BackendControlError::UnsupportedCapability { provider, capability })
            if provider.0 == LOCAL_PROVIDER_KIND && capability == "pause"
    ));
}

#[test]
fn local_create_and_attach_builds_active_backend() {
    let provider = local_backend_provider();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let workspace_root = BackendPath::from_raw(
        std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .to_string(),
    );

    let instance = runtime
        .block_on(provider.create_sandbox(BackendCreateRequest {
            requested_backend_id: None,
            session_id: "session".to_string(),
            conversation_id: None,
            workspace_root,
            provider_options: Value::Object(Map::new()),
            resource_limits: BackendResourceLimits::default(),
            metadata: Value::Null,
        }))
        .expect("create local instance");

    assert_eq!(instance.state, BackendLifecycleState::Active);
    assert_eq!(instance.provider.0, LOCAL_PROVIDER_KIND);

    let backend = runtime
        .block_on(provider.attach(instance))
        .expect("attach local backend");
    assert_eq!(backend.backend_id(), LOCAL_PROVIDER_KIND);
}

#[test]
fn local_inspect_uses_workspace_metadata_when_present() {
    let provider = local_backend_provider();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let mut metadata = Map::new();
    metadata.insert(
        LOCAL_METADATA_WORKSPACE_ROOT.to_string(),
        Value::String("/path/that/should/not/exist/xiaoo-local-provider".to_string()),
    );

    let status = runtime
        .block_on(provider.inspect(BackendInspectRequest {
            backend_id: Some(BackendId("local:test".to_string())),
            instance_id: None,
            snapshot_id: Some(BackendSnapshotId("snapshot".to_string())),
            metadata: Value::Object(metadata),
        }))
        .expect("inspect local backend");

    assert_eq!(status.state, BackendLifecycleState::Failed);
    assert!(status.last_error.is_some());
}
