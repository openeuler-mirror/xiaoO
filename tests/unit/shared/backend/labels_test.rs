use super::*;

#[test]
fn state_label_covers_all_variants() {
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

#[test]
fn endpoint_label_none() {
    assert!(backend_endpoint_str(None).is_none());
}

#[test]
fn endpoint_label_local() {
    assert_eq!(
        backend_endpoint_str(Some(BackendEndpoint::Local)).as_deref(),
        Some("local")
    );
}

#[test]
fn endpoint_label_tcp() {
    let s = backend_endpoint_str(Some(BackendEndpoint::Tcp {
        host: "127.0.0.1".to_string(),
        port: 8080,
    }));
    assert_eq!(s.as_deref(), Some("tcp://127.0.0.1:8080"));
}

#[test]
fn endpoint_label_unix() {
    let s = backend_endpoint_str(Some(BackendEndpoint::UnixSocket {
        path: "/run/x.sock".to_string(),
    }));
    assert_eq!(s.as_deref(), Some("unix:/run/x.sock"));
}

#[test]
fn endpoint_label_provider_handle() {
    let s = backend_endpoint_str(Some(BackendEndpoint::ProviderHandle {
        value: serde_json::Value::String("abc".to_string()),
    }));
    assert_eq!(s.as_deref(), Some("provider:\"abc\""));
}
