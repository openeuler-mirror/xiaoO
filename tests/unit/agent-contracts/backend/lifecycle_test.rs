use super::*;
use crate::backend::PathNamespace;

#[test]
fn create_starts_from_no_state_and_becomes_active() {
    let started =
        BackendLifecycleStateMachine::begin(None, BackendLifecycleOperation::CreateSandbox)
            .expect("create can start");
    assert_eq!(started, BackendLifecycleState::Creating);

    let completed = BackendLifecycleStateMachine::complete_success(
        started,
        BackendLifecycleOperation::CreateSandbox,
    )
    .expect("create completes");
    assert_eq!(completed, BackendLifecycleState::Active);
}

#[test]
fn pause_requires_active_and_becomes_paused() {
    let rejected = BackendLifecycleStateMachine::begin(
        Some(BackendLifecycleState::Paused),
        BackendLifecycleOperation::Pause,
    );
    assert!(matches!(
        rejected,
        Err(BackendControlError::InvalidState { .. })
    ));

    let started = BackendLifecycleStateMachine::begin(
        Some(BackendLifecycleState::Active),
        BackendLifecycleOperation::Pause,
    )
    .expect("pause can start");
    assert_eq!(started, BackendLifecycleState::Pausing);

    let completed =
        BackendLifecycleStateMachine::complete_success(started, BackendLifecycleOperation::Pause)
            .expect("pause completes");
    assert_eq!(completed, BackendLifecycleState::Paused);
}

#[test]
fn load_starts_from_paused_or_standalone_snapshot() {
    let from_paused = BackendLifecycleStateMachine::begin(
        Some(BackendLifecycleState::Paused),
        BackendLifecycleOperation::Load,
    )
    .expect("load from paused can start");
    assert_eq!(from_paused, BackendLifecycleState::Loading);

    let from_snapshot = BackendLifecycleStateMachine::begin(None, BackendLifecycleOperation::Load)
        .expect("load from standalone snapshot can start");
    assert_eq!(from_snapshot, BackendLifecycleState::Loading);
}

#[test]
fn delete_is_idempotent_for_deleted_state() {
    let started = BackendLifecycleStateMachine::begin(
        Some(BackendLifecycleState::Deleted),
        BackendLifecycleOperation::Delete,
    )
    .expect("delete can be repeated");
    assert_eq!(started, BackendLifecycleState::Deleted);

    let completed =
        BackendLifecycleStateMachine::complete_success(started, BackendLifecycleOperation::Delete)
            .expect("repeated delete completes");
    assert_eq!(completed, BackendLifecycleState::Deleted);
}

#[test]
fn failed_active_operation_enters_failed_state() {
    let started = BackendLifecycleStateMachine::begin(
        Some(BackendLifecycleState::Active),
        BackendLifecycleOperation::Delete,
    )
    .expect("delete can start");

    let failed =
        BackendLifecycleStateMachine::complete_failure(started, BackendLifecycleOperation::Delete)
            .expect("delete can fail");
    assert_eq!(failed, BackendLifecycleState::Failed);
}

#[test]
fn inspect_is_allowed_from_unknown_cache() {
    let started = BackendLifecycleStateMachine::begin(None, BackendLifecycleOperation::Inspect)
        .expect("inspect can start without cached state");
    assert_eq!(started, BackendLifecycleState::Unknown);

    let reconciled =
        BackendLifecycleStateMachine::reconcile_inspected_state(BackendLifecycleState::Active)
            .expect("inspect reconciles provider truth");
    assert_eq!(reconciled, BackendLifecycleState::Active);
}

#[test]
fn inspect_request_requires_at_least_one_target() {
    let request = BackendInspectRequest {
        backend_id: None,
        instance_id: None,
        snapshot_id: None,
        metadata: Value::Null,
    };
    assert!(!request.has_target());

    let request = BackendInspectRequest {
        backend_id: Some(BackendId("backend".to_string())),
        instance_id: None,
        snapshot_id: None,
        metadata: Value::Null,
    };
    assert!(request.has_target());
}

#[test]
fn backend_instance_serializes_attributed_workspace_root_as_object() {
    let instance = sample_backend_instance(BackendPath::new(
        "sess_01J_backend_root",
        PathNamespace::Workspace,
        "/home/user/project",
        "",
    ));
    let json = serde_json::to_value(&instance).unwrap();
    let root = &json["workspace_root"];
    assert_eq!(root["backend_id"], "sess_01J_backend_root");
    assert_eq!(root["namespace"], "workspace");
    assert_eq!(root["relative_path"], "");
    assert_eq!(root["native_path"], "/home/user/project");
    let back: BackendInstance = serde_json::from_value(json).unwrap();
    assert_eq!(back, instance);
}

#[test]
fn backend_instance_round_trips_legacy_string_workspace_root() {
    // Legacy writers emitted a plain string for the old `String` newtype.
    let instance = sample_backend_instance(BackendPath::from_raw("/home/user/project"));
    let json = serde_json::to_value(&instance).unwrap();
    assert_eq!(json["workspace_root"], "/home/user/project");
    let back: BackendInstance = serde_json::from_value(json).unwrap();
    assert_eq!(back, instance);
}

#[test]
fn backend_create_request_round_trips_both_workspace_root_forms() {
    // New writer -> new reader: attributed object form keeps metadata.
    let attributed = BackendCreateRequest {
        requested_backend_id: None,
        session_id: "sess_01J_create".to_string(),
        conversation_id: None,
        workspace_root: BackendPath::new(
            "sess_01J_create",
            PathNamespace::Temp,
            "/tmp/xiaoo-bash-output-1",
            "xiaoo-bash-output-1",
        ),
        provider_options: Value::Null,
        resource_limits: Default::default(),
        metadata: Value::Null,
    };
    let value = serde_json::to_value(&attributed).unwrap();
    assert_eq!(value["workspace_root"]["namespace"], "temp");
    let back: BackendCreateRequest = serde_json::from_value(value).unwrap();
    assert_eq!(back, attributed);

    // Old writer -> new reader: legacy string workspace_root still parses.
    let legacy: serde_json::Value = serde_json::from_str(
        r#"{"requested_backend_id":null,"session_id":"sess_legacy","conversation_id":null,"workspace_root":"/home/user/project","provider_options":null,"resource_limits":{},"metadata":null}"#,
    )
    .unwrap();
    let loaded: BackendCreateRequest = serde_json::from_value(legacy).unwrap();
    assert_eq!(
        loaded.workspace_root,
        BackendPath::from_raw("/home/user/project")
    );
}

fn sample_backend_instance(workspace_root: BackendPath) -> BackendInstance {
    BackendInstance {
        backend_id: BackendId("sess_01J_backend".to_string()),
        provider: BackendProviderKind("e2b".to_string()),
        instance_id: BackendInstanceId("inst_01J".to_string()),
        session_id: "sess_01J_backend".to_string(),
        state: BackendLifecycleState::Active,
        workspace_root,
        endpoint: Some(BackendEndpoint::Local),
        snapshot: None,
        capabilities: BackendRuntimeCapabilities::default(),
        resources: BackendResourceAllocation::default(),
        metadata: Value::Null,
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_000_001,
    }
}
