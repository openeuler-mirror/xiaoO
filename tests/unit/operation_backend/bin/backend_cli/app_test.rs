use super::*;

fn config_for(workspace: &std::path::Path) -> OperationBackendConfig {
    OperationBackendConfig::new(
        "local",
        json!({ "workspace_root": workspace.display().to_string() }),
    )
}

#[test]
fn parse_cli_args_supports_both_invocation_forms() {
    let config_form = parse_cli_args(&[
        "--config".to_string(),
        "/tmp/c.json".to_string(),
        "request".to_string(),
        "--json".to_string(),
        r#"{"kind":"stat","args":{"path":"/tmp"}}"#.to_string(),
    ])
    .expect("config form parses");
    match config_form {
        Invocation::ConfigFile {
            config_path,
            request_json,
        } => {
            assert_eq!(config_path, "/tmp/c.json");
            assert!(request_json.contains("stat"));
        }
        Invocation::StdinEnvelope => panic!("wrong variant"),
    }

    let stdin_form =
        parse_cli_args(&["request".to_string(), "--stdin".to_string()]).expect("stdin form parses");
    assert!(matches!(stdin_form, Invocation::StdinEnvelope));

    // Extra arguments after --stdin are rejected loudly.
    assert!(parse_cli_args(&[
        "request".to_string(),
        "--stdin".to_string(),
        "extra".to_string()
    ])
    .is_err());
    // Malformed classic forms still hit the usage error.
    assert!(parse_cli_args(&[]).is_err());
    assert!(parse_cli_args(&[
        "--config".to_string(),
        "/tmp/c.json".to_string(),
        "request".to_string(),
        "--json".to_string(),
    ])
    .is_err());
}

#[test]
fn decode_write_content_enforces_exactly_one_channel() {
    use base64::Engine as _;

    // Plain UTF-8 content.
    assert_eq!(
        decode_write_content(Some("hello".to_string()), None).unwrap(),
        b"hello".to_vec()
    );

    // Binary-safe channel: arbitrary (non-UTF-8) bytes round-trip.
    let payload: Vec<u8> = vec![0x00, 0x01, 0xff, 0xfe, 0x80, b'a', 0x7f];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
    assert_eq!(decode_write_content(None, Some(encoded)).unwrap(), payload);

    // Mutually exclusive / required.
    assert!(decode_write_content(Some("a".to_string()), Some("YQ==".to_string())).is_err());
    assert!(decode_write_content(None, None).is_err());
    // Invalid base64 surfaces a decoding error.
    assert!(decode_write_content(None, Some("!!not-base64!!".to_string())).is_err());
}

#[tokio::test]
async fn stdin_envelope_write_bytes_round_trips_binary_content() {
    use base64::Engine as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let payload: Vec<u8> = vec![0x00, 0xde, 0xad, 0xbe, 0xef, 0xff, 0x0a, 0x0d, b'\n'];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);

    // Envelope: backend section (config-file `backend` shape) + request
    // (classic `--json` shape) in one document.
    let envelope = json!({
        "backend": { "kind": "local", "options": { "workspace_root": workspace.display().to_string() } },
        "request": {
            "kind": "write_bytes",
            "args": {
                "path": workspace.join("blob.bin").display().to_string(),
                "content_b64": encoded,
                "mode": "atomic_overwrite"
            }
        }
    });
    let parsed: RequestEnvelope = serde_json::from_value(envelope).expect("envelope parses");
    let config = OperationBackendConfig::new(parsed.backend.kind, parsed.backend.options);

    execute_request(config, parsed.request)
        .await
        .expect("envelope execution");

    let on_disk = std::fs::read(workspace.join("blob.bin")).expect("written file");
    assert_eq!(on_disk, payload, "bytes must round-trip exactly");
}

#[tokio::test]
async fn stdin_envelope_create_dir_all_creates_nested_directories() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let target = workspace.join("a/b/c");
    let envelope = json!({
        "backend": { "kind": "local", "options": { "workspace_root": workspace.display().to_string() } },
        "request": {
            "kind": "create_dir_all",
            "args": { "path": target.display().to_string() }
        }
    });
    let parsed: RequestEnvelope = serde_json::from_value(envelope).unwrap();
    let config = OperationBackendConfig::new(parsed.backend.kind, parsed.backend.options);

    execute_request(config, parsed.request)
        .await
        .expect("envelope execution");

    assert!(target.is_dir());
}

#[tokio::test]
async fn envelope_write_bytes_without_content_reports_a_request_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let envelope = json!({
        "backend": { "kind": "local", "options": { "workspace_root": workspace.display().to_string() } },
        "request": {
            "kind": "write_bytes",
            "args": { "path": workspace.join("x.txt").display().to_string(), "mode": "overwrite" }
        }
    });
    let parsed: RequestEnvelope = serde_json::from_value(envelope).unwrap();
    let config = config_for(&workspace);

    let error = execute_request(config, parsed.request).await.unwrap_err();
    assert!(
        error.contains("content"),
        "error must mention the missing content channel: {error}"
    );
    assert!(!workspace.join("x.txt").exists());
}

#[tokio::test]
async fn classic_config_file_request_still_writes_plain_content() {
    // Regression for the `--config <path> request --json <json>` form: the
    // plain string content channel keeps working unchanged.
    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let request: BackendCliRequest = serde_json::from_value(json!({
        "kind": "write_bytes",
        "args": {
            "path": workspace.join("plain.txt").display().to_string(),
            "content": "plain text\n",
            "mode": "overwrite"
        }
    }))
    .unwrap();

    execute_request(config_for(&workspace), request)
        .await
        .expect("classic execution");

    assert_eq!(
        std::fs::read_to_string(workspace.join("plain.txt")).unwrap(),
        "plain text\n"
    );
}

#[tokio::test]
async fn envelope_large_base64_content_executes() {
    // A >1MB payload rides inside the envelope document (read from stdin,
    // never an argv entry), so it is not bound by the ~128KB single-arg
    // kernel limit. This verifies the large base64 body parses and
    // executes through the envelope shape; staying off argv is structural.
    use base64::Engine as _;

    let temp = tempfile::tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    let payload = vec![0x5au8; 1_100_000];
    let encoded = base64::engine::general_purpose::STANDARD.encode(&payload);
    let envelope = json!({
        "backend": { "kind": "local", "options": { "workspace_root": workspace.display().to_string() } },
        "request": {
            "kind": "write_bytes",
            "args": {
                "path": workspace.join("big.bin").display().to_string(),
                "content_b64": encoded,
                "mode": "overwrite"
            }
        }
    });
    let parsed: RequestEnvelope = serde_json::from_value(envelope).unwrap();
    let config = OperationBackendConfig::new(parsed.backend.kind, parsed.backend.options);

    execute_request(config, parsed.request)
        .await
        .expect("large envelope execution");

    assert_eq!(std::fs::read(workspace.join("big.bin")).unwrap(), payload);
}
