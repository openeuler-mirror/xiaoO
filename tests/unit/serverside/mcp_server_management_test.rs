use super::mcp_server_report;
use crate::daemon_config::DaemonConfig;
use std::fs;
use tempfile::TempDir;

#[test]
fn reports_disabled_server_without_requiring_secrets() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("config.toml");
    fs::write(
        &path,
        "[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[mcp_server]\nenabled = false\n",
    )
    .expect("write config");
    let report = mcp_server_report(&DaemonConfig::load_from(&path).expect("load config"));
    assert!(!report.enabled);
    assert!(report.valid);
    assert!(report.errors.is_empty());
}

#[test]
fn reports_enabled_server_preflight_errors_without_creating_workspace() {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("config.toml");
    let workspace = temp.path().join("not-created");
    fs::write(
        &path,
        format!("[llm]\nprovider = \"ollama\"\nmodel = \"test\"\n\n[mcp_server]\nenabled = true\nidle_timeout_secs = 0\n\n[mcp_server.chatbot]\nbearer_token_env = \"XIAOO_TEST_MISSING_CHATBOT\"\nworkspace = \"{}\"\n\n[mcp_server.agent]\nbearer_token_env = \"XIAOO_TEST_MISSING_AGENT\"\nagent_role = \"missing\"\n", workspace.display()),
    ).expect("write config");
    let report = mcp_server_report(&DaemonConfig::load_from(&path).expect("load config"));
    assert!(report.enabled);
    assert!(!report.valid);
    assert_eq!(report.chatbot.workspace_state, "missing");
    assert!(!workspace.exists());
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "missing_secret"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "unknown_role"));
    assert!(report
        .errors
        .iter()
        .any(|error| error.code == "out_of_range"));
}
