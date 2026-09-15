use super::*;

#[test]
fn legacy_string_deserializes_to_unattributed_path() {
    let path: BackendPath = serde_json::from_str(r#""/sandbox/tmp/bash-output-1""#).unwrap();
    assert_eq!(path, BackendPath::from_raw("/sandbox/tmp/bash-output-1"));
    assert!(path.backend_id.is_empty());
    assert_eq!(path.namespace, PathNamespace::Uncategorized);
    assert_eq!(path.native(), "/sandbox/tmp/bash-output-1");
}

#[test]
fn unattributed_path_serializes_as_legacy_string() {
    let path = BackendPath::from_raw("/sandbox/tmp/bash-output-1");
    let json = serde_json::to_string(&path).unwrap();
    assert_eq!(json, r#""/sandbox/tmp/bash-output-1""#);
    let back: BackendPath = serde_json::from_str(&json).unwrap();
    assert_eq!(back, path);
    // Display stays byte-for-byte compatible with the old String newtype.
    assert_eq!(path.to_string(), "/sandbox/tmp/bash-output-1");
}

#[test]
fn attributed_path_serializes_as_object_and_round_trips() {
    let path = BackendPath::new(
        "sess_01J_test_attrs",
        PathNamespace::Temp,
        "/tmp/xiaoo-bash-output-abc",
        "xiaoo-bash-output-abc",
    );
    let json = serde_json::to_string(&path).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["backend_id"], "sess_01J_test_attrs");
    assert_eq!(value["namespace"], "temp");
    assert_eq!(value["relative_path"], "xiaoo-bash-output-abc");
    assert_eq!(value["native_path"], "/tmp/xiaoo-bash-output-abc");
    let back: BackendPath = serde_json::from_str(&json).unwrap();
    assert_eq!(back, path);
}

#[test]
fn owned_by_accepts_own_and_raw_paths_rejects_foreign() {
    assert!(BackendPath::from_raw("/tmp/spill.txt")
        .assert_owned_by("backend-a")
        .is_ok());

    let own = BackendPath::new(
        "backend-a",
        PathNamespace::Temp,
        "/tmp/spill.txt",
        "spill.txt",
    );
    assert!(own.assert_owned_by("backend-a").is_ok());

    let foreign = BackendPath::new(
        "backend-b",
        PathNamespace::Temp,
        "/tmp/spill.txt",
        "spill.txt",
    );
    assert!(matches!(
        foreign.assert_owned_by("backend-a"),
        Err(OperationError::PermissionDenied { .. })
    ));
}
