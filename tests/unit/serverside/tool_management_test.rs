use super::tool_catalog;
use crate::daemon_config::DaemonConfig;
use tempfile::TempDir;

#[tokio::test]
async fn reports_effective_tool_catalog_for_configured_backend() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[llm]
active_profile = "local"

[llm.profiles.local]
provider = "ollama"
model = "qwen3"

[server.operation_backend]
kind = "e2b"
"#,
    )
    .expect("write config");
    let config = DaemonConfig::load_from(&config_path).expect("load config");
    let report = tool_catalog(&config, temp.path())
        .await
        .expect("tool catalog");

    assert_eq!(report.schema_version, 1);
    let bash = report
        .tools
        .iter()
        .find(|tool| tool.name == "bash")
        .expect("bash tool");
    assert_eq!(bash.source, "builtin");
    assert!(bash.effect.side_effects);
    assert!(report.tools.iter().all(|tool| tool.source != "custom"));
}
