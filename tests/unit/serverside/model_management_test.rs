use super::configured_profile;
use crate::daemon_config::DaemonConfig;
use tempfile::TempDir;

#[test]
fn resolves_disabled_profile_for_pre_enable_testing() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[llm]
active_profile = "active"

[llm.profiles.active]
provider = "ollama"
model = "qwen3"

[llm.profiles.candidate]
enabled = false
provider = "openai-compatible"
model = "candidate-model"
api_base = "https://example.com/v1"
"#,
    )
    .expect("write config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");
    let profile = configured_profile(&config, "candidate").expect("resolve profile");
    assert!(!profile.enabled);
    assert_eq!(profile.model, "candidate-model");
}

#[test]
fn rejects_unknown_profile() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[llm]
active_profile = "active"

[llm.profiles.active]
provider = "ollama"
model = "qwen3"
"#,
    )
    .expect("write config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");
    let error = configured_profile(&config, "missing").expect_err("profile must be missing");
    assert!(error
        .to_string()
        .contains("model profile `missing` does not exist"));
}
