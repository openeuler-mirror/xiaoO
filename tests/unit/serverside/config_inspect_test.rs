use super::{inspect_config_file, ConfigInspectOverrides};
use serde_json::json;
use tempfile::TempDir;

#[test]
fn reports_sources_defaults_overrides_and_redacts_secrets() {
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

[skills]
allow_scripts = true

[http]
bearer_token = "do-not-print"
"#,
    )
    .expect("write config");

    let report = inspect_config_file(
        &path,
        &ConfigInspectOverrides {
            daemon_host: "127.0.0.1".to_string(),
            daemon_port: 0,
            no_dashboard: true,
            ..Default::default()
        },
    );

    assert!(report.valid, "{:?}", report.errors);
    assert_eq!(report.values["skills.allow_scripts"].source, "config_file");
    assert_eq!(
        report.values["channels.interaction_timeout_secs"].source,
        "default"
    );
    assert_eq!(
        report.values["http.dashboard.enabled"].source,
        "command_line"
    );
    assert_eq!(report.values["http.dashboard.enabled"].value, json!(false));
    assert_eq!(
        report.values["http.bearer_token"].value,
        json!("<redacted>")
    );
    assert_eq!(report.config["http"]["bearer_token"], "<redacted>");
    assert_eq!(report.profiles[0].api_key_status, "not_required");
}

#[test]
fn reports_api_key_available_from_the_selected_config_secret_store() {
    let temp = TempDir::new().expect("tempdir");
    let path = temp.path().join("config.toml");
    let env_name = "XIAOO_INSPECT_ENCRYPTED_KEY_42";
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

    let report = inspect_config_file(&path, &ConfigInspectOverrides::default());

    assert!(report.valid, "{:?}", report.errors);
    assert_eq!(report.profiles[0].api_key_status, "available");
}
