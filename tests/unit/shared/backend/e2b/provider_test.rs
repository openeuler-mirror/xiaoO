use super::*;
use agent_contracts::backend::capability::exec::ExecRequest;
use agent_contracts::backend::capability::filesystem::{WriteBytesRequest, WriteMode};
use std::cell::Cell;
use std::future::ready;

#[tokio::test]
#[ignore = "requires E2B_API_KEY and creates a real E2B sandbox"]
async fn live_structured_grep_executes_command_with_args() {
    assert!(
        std::env::var_os("E2B_API_KEY").is_some(),
        "E2B_API_KEY must be set"
    );

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let created = create_backend(E2bCreateBackendInput {
        backend_id: BackendId(format!("e2b-live-grep:{suffix}")),
        session_id_for_instance: format!("e2b-live-grep-session:{suffix}"),
        workspace_root_text: DEFAULT_WORKSPACE_ROOT.to_string(),
        provider_options: json!({
            "api_key_env": "E2B_API_KEY",
            "template_id": "base",
            "timeout_secs": 300,
            "default_shell": "/bin/sh"
        }),
        resource_limits: Default::default(),
        metadata: json!({"purpose": "structured grep live smoke"}),
        bootstrap: None,
    })
    .await
    .expect("create live E2B backend");
    let backend = created.backend;
    let smoke_path = BackendPath::from_raw(format!("{DEFAULT_WORKSPACE_ROOT}/grep-smoke.py"));

    let smoke_result: Result<String, String> = async {
        backend
            .files()
            .write_bytes(WriteBytesRequest {
                path: smoke_path,
                content: b"watt = watts = W = Quantity(\"watt\")\n".to_vec(),
                mode: WriteMode::Overwrite,
            })
            .await
            .map_err(|error| format!("write smoke fixture: {error}"))?;

        let output = backend
            .exec()
            .exec(ExecRequest {
                command: "grep".to_string(),
                args: vec![
                    "-P".to_string(),
                    "-n".to_string(),
                    "-e".to_string(),
                    r"W\s*=".to_string(),
                    "grep-smoke.py".to_string(),
                ],
                cwd: Some(BackendPath::from_raw(DEFAULT_WORKSPACE_ROOT.to_string())),
                timeout_ms: Some(30_000),
                ..Default::default()
            })
            .await
            .map_err(|error| format!("execute structured grep: {error}"))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.exit_code != Some(0) {
            return Err(format!(
                "grep exited with {:?}; stderr: {stderr}",
                output.exit_code
            ));
        }
        if !stdout.contains("1:watt = watts = W = Quantity") {
            return Err(format!("unexpected grep stdout: {stdout:?}"));
        }
        Ok(stdout)
    }
    .await;

    let cleanup_result = backend.shutdown().await;
    cleanup_result.expect("delete live E2B sandbox");
    let stdout = smoke_result.expect("structured grep smoke");
    eprintln!("live E2B structured grep passed: {}", stdout.trim());
}

#[tokio::test]
async fn workspace_initialization_retries_transient_failures() {
    let attempts = Cell::new(0usize);
    let sleeps = Cell::new(0usize);

    let output = retry_workspace_initialization(
        "sandbox-test",
        || {
            let attempt = attempts.get() + 1;
            attempts.set(attempt);
            ready(if attempt < 3 {
                Err(super::super::exec::E2bExecFailure::retryable_for_test(
                    "temporary reset",
                ))
            } else {
                Ok(super::super::exec::E2bExecOutput {
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                    exit_code: Some(0),
                    timed_out: false,
                })
            })
        },
        |_| {
            sleeps.set(sleeps.get() + 1);
            ready(())
        },
    )
    .await
    .expect("third attempt should succeed");

    assert_eq!(output.exit_code, Some(0));
    assert_eq!(attempts.get(), 3);
    assert_eq!(sleeps.get(), 2);
}

#[test]
fn workspace_backoff_is_bounded_and_jittered() {
    for attempt in 1..=WORKSPACE_INIT_MAX_ATTEMPTS {
        let delay_ms = workspace_init_backoff(attempt).as_millis() as u64;
        let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
        let base = WORKSPACE_INIT_BASE_DELAY_MS
            .saturating_mul(2u64.saturating_pow(exponent))
            .min(WORKSPACE_INIT_MAX_DELAY_MS);
        assert!(delay_ms >= base - base / 5);
        assert!(delay_ms <= base + base / 5);
    }
}

#[test]
fn redacts_direct_api_key_from_metadata() {
    let options = parse_options(&json!({
        "api_key": "secret",
        "template_id": "base",
        "envVars": {"TOKEN": "secret"}
    }))
    .expect("options");

    let redacted = redacted_provider_options(
        &json!({
            "api_key": "secret",
            "template_id": "base",
            "envVars": {"TOKEN": "secret"}
        }),
        &options,
    );

    let object = redacted.as_object().expect("object");
    assert!(!object.contains_key("api_key"));
    assert!(!object.contains_key("envVars"));
    assert_eq!(object.get("api_key_configured"), Some(&Value::Bool(true)));
}

#[test]
fn default_template_is_base() {
    let options = parse_options(&json!({})).expect("options");
    assert_eq!(
        options
            .template_id
            .as_deref()
            .unwrap_or(DEFAULT_TEMPLATE_ID),
        "base"
    );
}

#[test]
fn connection_options_default_to_e2b_cloud() {
    let options = parse_options(&json!({})).expect("options");
    let connection =
        resolve_connection_options_from_values(&options, None, None).expect("connection options");

    assert_eq!(connection.api_base, DEFAULT_API_BASE);
    assert_eq!(connection.sandbox_domain, DEFAULT_DOMAIN);
}

#[test]
fn connection_options_derive_api_url_from_self_hosted_domain() {
    let options = parse_options(&json!({})).expect("options");
    let connection =
        resolve_connection_options_from_values(&options, None, Some(" self-hosted.example.com. "))
            .expect("connection options");

    assert_eq!(connection.api_base, "https://api.self-hosted.example.com");
    assert_eq!(connection.sandbox_domain, "self-hosted.example.com");
}

#[test]
fn connection_options_accept_api_url_and_domain_environment_values() {
    let options = parse_options(&json!({})).expect("options");
    let connection = resolve_connection_options_from_values(
        &options,
        Some(" https://control.self-hosted.example.com/ "),
        Some("self-hosted.example.com"),
    )
    .expect("connection options");

    assert_eq!(
        connection.api_base,
        "https://control.self-hosted.example.com/"
    );
    assert_eq!(connection.sandbox_domain, "self-hosted.example.com");
}

#[test]
fn explicit_connection_options_override_environment_values() {
    let options = parse_options(&json!({
        "apiUrl": "https://control.internal.example.com/",
        "sandboxDomain": "sandboxes.internal.example.com"
    }))
    .expect("options");
    let connection = resolve_connection_options_from_values(
        &options,
        Some("https://api.from-env.example.com"),
        Some("from-env.example.com"),
    )
    .expect("connection options");

    assert_eq!(connection.api_base, "https://control.internal.example.com/");
    assert_eq!(connection.sandbox_domain, "sandboxes.internal.example.com");
}

#[test]
fn rejects_domain_with_scheme() {
    let options = parse_options(&json!({
        "domain": "https://self-hosted.example.com"
    }))
    .expect("options");
    let error = resolve_connection_options_from_values(&options, None, None)
        .expect_err("domain with a scheme must be rejected");

    assert!(error
        .to_string()
        .contains("hostname without scheme or path"));
}

#[test]
fn provider_handle_uses_configured_sandbox_domain() {
    let sandbox = CreateSandboxResponse {
        sandbox_id: "sandbox-test".to_string(),
        template_id: "base".to_string(),
        envd_access_token: Some("access-token".to_string()),
        traffic_access_token: None,
    };
    let endpoint = provider_handle(&sandbox, 49_983, "https", "self-hosted.example.com");
    let BackendEndpoint::ProviderHandle { value } = endpoint else {
        panic!("expected provider handle");
    };

    assert_eq!(
        value["envd_host"],
        "49983-sandbox-test.self-hosted.example.com"
    );
}

#[test]
fn encodes_template_id_as_single_path_segment() {
    assert_eq!(
        encode_path_segment("team/fork-test:default"),
        "team%2Ffork-test%3Adefault"
    );
}
