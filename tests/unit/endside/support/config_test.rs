use super::{require_tui_bootstrap_config, resolve_context_window, Config, TuiConfig};
use std::path::Path;
use tempfile::tempdir;

fn valid_config() -> Config {
    let mut config = Config::default();
    config.llm.provider = "openai".to_string();
    config.llm.model = "gpt-4o".to_string();
    config.llm.max_tokens = 128000;
    config
}

#[test]
fn tui_bootstrap_requires_config_file() {
    let error = require_tui_bootstrap_config(None, Path::new("/tmp/missing.toml"))
        .expect_err("missing config should fail");
    assert!(
        error.to_string().contains("config file not found"),
        "unexpected error: {error}"
    );
}

#[test]
fn resolve_context_window_falls_back_to_provider_default() {
    let mut config = valid_config();
    config.llm.provider = "anthropic".to_string();

    assert_eq!(resolve_context_window(&config), Some(200_000));
}

#[test]
fn parses_memory_automation_config() {
    let config: Config = toml::from_str(
        r#"
[memory_automation]
enabled = true
server = "ram-a"
recall_top_k = 3
recall_token_budget = 128
context_messages = 2
queue_path = "/tmp/xiaoo-memory-queue.jsonl"
queue_capacity = 32
max_retries = 4
retry_backoff_ms = 50
allowed_agent_roles = ["main", "researcher"]
"#,
    )
    .expect("config should parse");

    assert!(config.memory_automation.enabled);
    assert_eq!(config.memory_automation.server, "ram-a");
    assert_eq!(config.memory_automation.recall_top_k, 3);
    assert_eq!(config.memory_automation.recall_token_budget, 128);
    assert_eq!(config.memory_automation.context_messages, 2);
    assert_eq!(config.memory_automation.queue_capacity, 32);
    assert_eq!(config.memory_automation.max_retries, 4);
    assert_eq!(config.memory_automation.retry_backoff_ms, 50);
    assert_eq!(
        config.memory_automation.allowed_agent_roles,
        vec!["main".to_string(), "researcher".to_string()]
    );
}

#[test]
fn tui_bootstrap_requires_default_agent_id_in_agents_list() {
    let mut config = valid_config();
    config.agents.default_agent_id = "missing".to_string();
    config.agents.list.push(super::AgentConfig {
        id: "main".to_string(),
        workspace_dir: None,
    });

    let error = require_tui_bootstrap_config(Some(config), Path::new("/tmp/config.toml"))
        .expect_err("default agent id mismatch should fail");
    assert!(
        error
            .to_string()
            .contains("agents.default_agent_id validation failed"),
        "unexpected error: {error}"
    );
}

#[test]
fn parses_agent_role_presets() {
    let config: Config = toml::from_str(
        r#"
[llm]
provider = "openai"
model = "gpt-4o"
max_tokens = 128000
context_window = 128000

[agent.code-reviewer]
description = "Reviews code for best practices and potential issues"
prompt = "You are a code reviewer."
max_turns = 3

[agent.code-reviewer.tools]
file_write = false
file_edit = false
"#,
    )
    .expect("agent role config should parse");

    let role = config
        .agent_role("code-reviewer")
        .expect("code-reviewer role should exist");
    assert_eq!(
        role.description,
        "Reviews code for best practices and potential issues"
    );
    assert_eq!(role.prompt.as_deref(), Some("You are a code reviewer."));
    assert_eq!(role.max_turns, Some(3));
    assert_eq!(role.tools.get("file_write"), Some(&false));
    assert_eq!(role.tools.get("file_edit"), Some(&false));
}

#[test]
fn save_preserves_daemon_owned_top_level_sections() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
provider = "openai"
model = "gpt-4o"

[mcp_server]
enabled = true

[mcp_server.chatbot]
bearer_token_env = "XIAOO_MCP_CHATBOT_TOKEN"

[server.operation_backend]
kind = "local"
"#,
    )
    .expect("write config");

    let mut config = Config::load_from(&path).expect("load config");
    config.llm.model = "gpt-4.1".to_string();
    config.save_to(&path).expect("save config");

    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).expect("read persisted config"))
            .expect("parse persisted config");
    assert_eq!(persisted["mcp_server"]["enabled"].as_bool(), Some(true));
    assert_eq!(
        persisted["mcp_server"]["chatbot"]["bearer_token_env"].as_str(),
        Some("XIAOO_MCP_CHATBOT_TOKEN")
    );
    assert_eq!(
        persisted["server"]["operation_backend"]["kind"].as_str(),
        Some("local")
    );
}

#[test]
fn imported_json_mcp_servers_are_runtime_only_when_tui_saves() {
    let temp = tempdir().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let json_path = temp.path().join("mcp.json");
    std::fs::write(
        &config_path,
        r#"
[llm]
provider = "openai"
model = "gpt-4o"

[[mcp.servers]]
name = "toml-server"
transport = "stdio"
command = "toml-server"
"#,
    )
    .expect("write TOML config");
    std::fs::write(
        &json_path,
        r#"{"mcpServers":{"json-server":{"transport":"stdio","command":"json-server"}}}"#,
    )
    .expect("write JSON config");

    let mut config = Config::load_from(&config_path).expect("load TOML config");
    config
        .load_runtime_mcp_servers(
            Some(&json_path),
            temp.path(),
            Some(temp.path()),
            &config_path,
        )
        .expect("merge JSON MCP servers");
    assert_eq!(
        config
            .mcp_servers()
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["toml-server", "json-server"]
    );

    config.llm.model = "gpt-4.1".to_string();
    config.save_to(&config_path).expect("save TUI config");
    let persisted = Config::load_from(&config_path).expect("reload TOML config");
    assert_eq!(persisted.mcp.servers.len(), 1);
    assert_eq!(persisted.mcp.servers[0].name, "toml-server");
}

#[test]
fn tui_bootstrap_adds_builtin_plan_agent_role() {
    let config = require_tui_bootstrap_config(Some(valid_config()), Path::new("/tmp/config.toml"))
        .expect("valid config should bootstrap");
    let plan = config
        .agent_role("plan")
        .expect("builtin plan role should exist");

    assert_eq!(plan.max_turns, None);
    assert!(plan
        .prompt
        .as_deref()
        .unwrap_or_default()
        .contains("todo_write"));
    assert_eq!(plan.tools.get("bash"), Some(&false));
}

#[test]
fn tui_bootstrap_rejects_builtin_plan_override() {
    let mut config = valid_config();
    config.agent.insert(
        "plan".to_string(),
        super::AgentRoleConfig {
            description: "override".to_string(),
            ..super::AgentRoleConfig::default()
        },
    );

    let error = require_tui_bootstrap_config(Some(config), Path::new("/tmp/config.toml"))
        .expect_err("builtin plan override should fail");
    assert!(format!("{error:?}").contains("builtin"));
}

/// The local TUI defaults `redact_secrets_display` to `false` (show the
/// transcript as-is); users opt into shoulder-surf protection via
/// `[tui] redact_secrets_display = true`. This asymmetric default (vs
/// `FeatureFlags::default() = true`, which the daemon inherits) is what
/// makes the local TUI default-off while the remote path stays always-on.
#[test]
fn tui_config_default_does_not_redact_secrets() {
    assert_eq!(TuiConfig::default().redact_secrets_display, false);
    assert_eq!(Config::default().tui.redact_secrets_display, false);
}
