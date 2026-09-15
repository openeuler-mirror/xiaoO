use super::{agent_catalog, test_agent_startup};
use crate::daemon_config::DaemonConfig;

#[test]
fn reports_profile_references_and_default_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"[llm]
active_profile = "local"
[llm.profiles.local]
provider = "ollama"
model = "qwen"
[agents]
default_agent_id = "review"
[[agents.list]]
id = "review"
profile_id = "local"
system_prompt = "Review carefully"
"#,
    )
    .expect("config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");
    let report = agent_catalog(&config).expect("catalog");
    assert_eq!(report.default_agent_id, "review");
    assert!(report.agents[0].default);
    assert!(report.agents[0].profile_available);
    assert_eq!(
        report.agents[0].system_prompt.as_deref(),
        Some("Review carefully")
    );
}

#[tokio::test]
async fn tests_selected_agent_runtime_initialization() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[llm]\nactive_profile = \"local\"\n[llm.profiles.local]\nprovider = \"ollama\"\nmodel = \"qwen\"\napi_base = \"http://127.0.0.1:11434\"\n[[agents.list]]\nid = \"main\"\nprofile_id = \"local\"\nworkspace = {}\n",
            serde_json::to_string(&workspace.display().to_string()).expect("path")
        ),
    )
    .expect("config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");
    let report = test_agent_startup(&config, "main").await;
    assert!(report.success, "{:?}", report.error);
    assert_eq!(report.profile_id.as_deref(), Some("local"));
    assert!(workspace.is_dir());

    let missing = test_agent_startup(&config, "missing").await;
    assert!(!missing.success);
    assert!(missing
        .error
        .as_deref()
        .is_some_and(|error| error.contains("does not exist")));
}
