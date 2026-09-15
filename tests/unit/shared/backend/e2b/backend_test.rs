use super::*;
use std::sync::Mutex;

fn test_state() -> E2bBackendState {
    E2bBackendState {
        backend_id: "e2b:test".to_string(),
        api_base: "https://api.e2b.test".to_string(),
        api_key: "test-key".to_string(),
        sandbox_id: "sandbox-test".to_string(),
        sandbox_domain: "e2b.test".to_string(),
        envd_access_token: None,
        envd_port: 49_983,
        envd_scheme: "https".to_string(),
        workspace_root: BackendPath::from_raw("/home/user/workspace".to_string()),
        home_dir: Some(BackendPath::from_raw("/home/user".to_string())),
        temp_root: BackendPath::from_raw("/tmp".to_string()),
        default_shell: None,
        username: None,
        envd_file_upload_multipart: false,
        http: reqwest::Client::new(),
        lifecycle: Mutex::new(E2bLifecycle::Active),
    }
}

#[test]
fn tag_backend_path_classifies_by_namespace() {
    let state = test_state();

    let workspace = state.tag_backend_path("/home/user/workspace/src/main.rs");
    assert_eq!(workspace.backend_id, "e2b:test");
    assert_eq!(workspace.namespace, PathNamespace::Workspace);
    assert_eq!(workspace.relative_path, "src/main.rs");
    assert_eq!(workspace.native(), "/home/user/workspace/src/main.rs");

    let home = state.tag_backend_path("/home/user/.config/app.json");
    assert_eq!(home.namespace, PathNamespace::Home);
    assert_eq!(home.relative_path, ".config/app.json");

    let temp = state.tag_backend_path("/tmp/spill.txt");
    assert_eq!(temp.namespace, PathNamespace::Temp);
    assert_eq!(temp.relative_path, "spill.txt");

    let outside = state.tag_backend_path("/var/log/syslog");
    assert_eq!(outside.namespace, PathNamespace::Uncategorized);
}

#[test]
fn resolve_and_owned_native_enforce_backend_ownership() {
    let state = test_state();
    let base = state.workspace_root.clone();

    let relative = state.resolve_backend_path("src/main.rs", &base).unwrap();
    assert_eq!(relative.namespace, PathNamespace::Workspace);
    assert_eq!(relative.relative_path, "src/main.rs");

    let home = state.resolve_backend_path("~/bin/tool", &base).unwrap();
    assert_eq!(home.namespace, PathNamespace::Home);
    assert_eq!(home.relative_path, "bin/tool");
    assert_eq!(home.native(), "/home/user/bin/tool");

    // Own and legacy (unattributed) paths are accepted.
    assert!(state.owned_native(&relative).is_ok());
    assert!(state
        .owned_native(&BackendPath::from_raw("/tmp/legacy"))
        .is_ok());

    // A path attributed to a different backend is rejected.
    let foreign = BackendPath::new(
        "other-backend",
        PathNamespace::Temp,
        "/tmp/spill.txt",
        "spill.txt",
    );
    assert!(matches!(
        state.owned_native(&foreign),
        Err(OperationError::PermissionDenied { .. })
    ));
}
