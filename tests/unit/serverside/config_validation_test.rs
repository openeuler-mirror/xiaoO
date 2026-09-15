use super::validate_config_file;
use tempfile::TempDir;

#[test]
fn rejects_missing_base_and_api_key_before_daemon_start() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
active_profile = "broken"

[llm.profiles.broken]
provider = "openai-compatible"
model = "test-model"
api_key_env = "XIAOO_VALIDATION_MISSING_KEY_42"
"#,
    )
    .expect("write config");

    let report = validate_config_file(&path);
    assert!(!report.valid);
    assert!(report.errors.iter().any(|issue| {
        issue.path == "llm.profiles.broken.api_base" && issue.code == "missing_api_base"
    }));
    assert!(report.errors.iter().any(|issue| {
        issue.path == "llm.profiles.broken.api_key_env" && issue.code == "missing_api_key"
    }));
}

#[test]
fn validates_every_enabled_profile() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
active_profile = "primary"

[llm.profiles.primary]
provider = "ollama"
model = "qwen"

[llm.profiles.secondary]
provider = "not-a-provider"
model = "other"

[llm.profiles.ignored]
enabled = false
provider = "also-unknown"
model = "ignored"
"#,
    )
    .expect("write config");

    let report = validate_config_file(&path);
    assert!(!report.valid);
    assert_eq!(report.errors.len(), 1);
    assert_eq!(report.errors[0].path, "llm.profiles.secondary.provider");
    assert_eq!(report.errors[0].code, "unknown_provider");
}

#[test]
fn accepts_valid_local_profile_without_api_key() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
active_profile = "local"

[llm.profiles.local]
provider = "ollama"
model = "qwen"
"#,
    )
    .expect("write config");

    let report = validate_config_file(&path);
    assert!(report.valid, "{:?}", report.errors);
}

#[test]
fn accepts_api_key_from_the_selected_config_secret_store() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    let env_name = "XIAOO_VALIDATION_ENCRYPTED_KEY_42";
    std::env::remove_var(env_name);
    std::fs::write(
        &path,
        format!(
            r#"
[llm]
active_profile = "remote"

[llm.profiles.remote]
provider = "openrouter"
model = "test-model"
api_key_env = "{env_name}"
"#,
        ),
    )
    .expect("write config");
    xiaoo_shared::llm_secrets::save_llm_secret(&path, env_name, "secret-key").expect("save secret");

    let report = validate_config_file(&path);
    assert!(report.valid, "{:?}", report.errors);
}

#[test]
fn rejects_invalid_compact_configuration_before_daemon_start() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[compact]\nwarning_ratio = 0.95\nauto_compact_ratio = 0.75\nblocking_ratio = 0.9\nsummary_llm_max_tokens = 0\n",
    )
    .expect("write config");

    let report = validate_config_file(&path);

    assert!(!report.valid);
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.code == "invalid_threshold_order"));
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.path == "compact.summary_llm_max_tokens"));
}

#[test]
fn rejects_missing_e2b_key_before_daemon_start() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[server.operation_backend]\nkind = \"e2b\"\n[server.operation_backend.options]\napi_key_env = \"XIAOO_TEST_MISSING_E2B_KEY_82\"\n",
    )
    .expect("write config");
    std::env::remove_var("XIAOO_TEST_MISSING_E2B_KEY_82");

    let report = validate_config_file(&path);

    assert!(!report.valid);
    assert!(report.errors.iter().any(|issue| {
        issue.path == "server.operation_backend.options.api_key_env"
            && issue.code == "missing_api_key"
    }));
}

#[test]
fn rejects_missing_channel_secret_before_daemon_start() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::env::remove_var("XIAOO_TEST_MISSING_TELEGRAM_VALIDATION");
    std::fs::write(
        &path,
        "[llm]\nprovider='ollama'\nmodel='test'\n\n[channels.telegram]\nenabled=true\ntransport='polling'\nbot_token_env='XIAOO_TEST_MISSING_TELEGRAM_VALIDATION'\n",
    )
    .expect("write config");

    let report = validate_config_file(&path);

    assert!(!report.valid);
    assert!(report.errors.iter().any(|issue| {
        issue.path == "channels.telegram.bot_token_env" && issue.code == "missing_secret"
    }));
}

#[test]
fn rejects_invalid_http_limits_and_missing_bearer_secret() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::env::remove_var("XIAOO_TEST_MISSING_HTTP_TOKEN");
    std::fs::write(
        &path,
        "[llm]\nprovider='ollama'\nmodel='test'\n\n[http]\nbearer_token_env='XIAOO_TEST_MISSING_HTTP_TOKEN'\n\n[http.rate_limit]\nenabled=true\nrequests_per_second=0\nburst=0\n\n[http.dashboard]\nhost=''\nport=0\n",
    )
    .expect("write config");

    let report = validate_config_file(&path);

    assert!(!report.valid);
    for path in [
        "http.bearer_token_env",
        "http.rate_limit.requests_per_second",
        "http.rate_limit.burst",
        "http.dashboard.host",
        "http.dashboard.port",
    ] {
        assert!(
            report.errors.iter().any(|issue| issue.path == path),
            "{path}"
        );
    }
}

#[test]
fn rejects_unknown_trace_backend_and_empty_database_path() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        "[llm]\nprovider='ollama'\nmodel='test'\n\n[trace]\nstorage_backend='unknown'\ndb_path=''\n",
    )
    .expect("write config");
    let report = validate_config_file(&path);
    assert!(!report.valid);
    assert!(report.errors.iter().any(|issue| {
        issue.path == "trace.storage_backend" && issue.code == "unsupported_backend"
    }));
    assert!(report
        .errors
        .iter()
        .any(|issue| issue.path == "trace.db_path" && issue.code == "required"));
}
