use super::{Cli, CliAction};
use std::path::PathBuf;

#[test]
fn parses_daemon_arguments() {
    let cli = Cli::parse(
        [
            "--config",
            "/tmp/demo.toml",
            "--host",
            "127.0.0.1",
            "--port",
            "18080",
            "--no-dashboard",
            "--ready-stdio",
            "--bearer-token-env",
            "XIAOO_CLIENT_DAEMON_TOKEN",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("cli should parse");

    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    assert_eq!(cli.host, "127.0.0.1");
    assert_eq!(cli.port, 18080);
    assert!(cli.no_dashboard);
    assert!(cli.ready_stdio);
    assert_eq!(
        cli.bearer_token_env.as_deref(),
        Some("XIAOO_CLIENT_DAEMON_TOKEN")
    );
    assert!(!cli.help);
    assert_eq!(cli.action, CliAction::Serve);
}

#[test]
fn parses_config_validate_command() {
    let cli = Cli::parse(
        ["config", "validate", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config validate should parse");

    assert_eq!(cli.action, CliAction::ValidateConfig);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_schema_command() {
    let cli = Cli::parse(["config", "schema"].into_iter().map(str::to_string))
        .expect("config schema should parse");

    assert_eq!(cli.action, CliAction::ConfigSchema);
}

#[test]
fn parses_config_providers_command() {
    let cli = Cli::parse(["config", "providers"].into_iter().map(str::to_string))
        .expect("config providers should parse");

    assert_eq!(cli.action, CliAction::ConfigProviders);
}

#[test]
fn parses_config_inspect_with_startup_overrides() {
    let cli = Cli::parse(
        [
            "config",
            "inspect",
            "--config",
            "/tmp/demo.toml",
            "--no-dashboard",
            "--host",
            "127.0.0.1",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config inspect should parse");

    assert_eq!(cli.action, CliAction::ConfigInspect);
    assert!(cli.no_dashboard);
    assert_eq!(cli.host, "127.0.0.1");
}

#[test]
fn parses_config_test_model_command() {
    let cli = Cli::parse(
        [
            "config",
            "test-model",
            "--profile",
            "qwen",
            "--config",
            "/tmp/demo.toml",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config test-model should parse");

    assert_eq!(cli.action, CliAction::ConfigTestModel);
    assert_eq!(cli.profile.as_deref(), Some("qwen"));
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_models_command() {
    let cli = Cli::parse(
        ["config", "models", "--profile", "qwen"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config models should parse");

    assert_eq!(cli.action, CliAction::ConfigModels);
    assert_eq!(cli.profile.as_deref(), Some("qwen"));
}

#[test]
fn parses_config_roles_command() {
    let cli = Cli::parse(
        ["config", "roles", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config roles should parse");

    assert_eq!(cli.action, CliAction::ConfigRoles);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_agents_command() {
    let cli = Cli::parse(
        ["config", "agents", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config agents should parse");

    assert_eq!(cli.action, CliAction::ConfigAgents);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_test_agent_command() {
    let cli = Cli::parse(
        ["config", "test-agent", "--agent", "review"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config test-agent should parse");

    assert_eq!(cli.action, CliAction::ConfigTestAgent);
    assert_eq!(cli.agent.as_deref(), Some("review"));
}

#[test]
fn parses_config_tools_command() {
    let cli = Cli::parse(
        ["config", "tools", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config tools should parse");

    assert_eq!(cli.action, CliAction::ConfigTools);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_custom_tools_command() {
    let cli = Cli::parse(
        ["config", "custom-tools", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config custom-tools should parse");

    assert_eq!(cli.action, CliAction::ConfigCustomTools);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_render_custom_tool_command() {
    let cli = Cli::parse(
        ["config", "render-custom-tool"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config render-custom-tool should parse");

    assert_eq!(cli.action, CliAction::ConfigRenderCustomTool);
}

#[test]
fn parses_config_test_custom_tool_command() {
    let cli = Cli::parse(
        [
            "config",
            "test-custom-tool",
            "--manifest",
            "/tmp/echo.toml",
            "--allow-effects",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config test-custom-tool should parse");

    assert_eq!(cli.action, CliAction::ConfigTestCustomTool);
    assert_eq!(cli.manifest, Some(PathBuf::from("/tmp/echo.toml")));
    assert!(cli.allow_effects);
}

#[test]
fn parses_config_skills_command() {
    let cli = Cli::parse(
        ["config", "skills", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config skills should parse");

    assert_eq!(cli.action, CliAction::ConfigSkills);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_hooks_command() {
    let cli = Cli::parse(
        ["config", "hooks", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config hooks should parse");

    assert_eq!(cli.action, CliAction::ConfigHooks);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_test_hook_command() {
    let cli = Cli::parse(
        [
            "config",
            "test-hook",
            "--hooker",
            "builtin_session_created_hooker",
            "--config",
            "/tmp/demo.toml",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config test-hook should parse");

    assert_eq!(cli.action, CliAction::ConfigTestHook);
    assert_eq!(
        cli.hooker.as_deref(),
        Some("builtin_session_created_hooker")
    );
}

#[test]
fn parses_config_mcp_command() {
    let cli = Cli::parse(
        [
            "config",
            "mcp",
            "--config",
            "/tmp/demo.toml",
            "--mcp-config",
            "/tmp/mcp.json",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config mcp should parse");

    assert_eq!(cli.action, CliAction::ConfigMcp);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    assert_eq!(cli.mcp_config, Some(PathBuf::from("/tmp/mcp.json")));
}

#[test]
fn parses_config_lsp_command() {
    let cli = Cli::parse(
        ["config", "lsp", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config lsp should parse");

    assert_eq!(cli.action, CliAction::ConfigLsp);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_lsp_action_commands() {
    let install = Cli::parse(
        ["config", "install-lsp", "--server", "gopls"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config install-lsp should parse");
    assert_eq!(install.action, CliAction::ConfigInstallLsp);
    assert_eq!(install.server.as_deref(), Some("gopls"));

    let test = Cli::parse(
        [
            "config",
            "test-lsp",
            "--server",
            "rust-analyzer",
            "--workspace",
            "/tmp/workspace",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config test-lsp should parse");
    assert_eq!(test.action, CliAction::ConfigTestLsp);
    assert_eq!(test.server.as_deref(), Some("rust-analyzer"));
    assert_eq!(test.workspace, Some(PathBuf::from("/tmp/workspace")));
}

#[test]
fn parses_config_mcp_server_command() {
    let cli = Cli::parse(
        ["config", "mcp-server", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config mcp-server should parse");

    assert_eq!(cli.action, CliAction::ConfigMcpServer);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_memory_command() {
    let cli = Cli::parse(
        ["config", "memory", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config memory should parse");

    assert_eq!(cli.action, CliAction::ConfigMemory);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_compact_command() {
    let cli = Cli::parse(
        ["config", "compact", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config compact should parse");

    assert_eq!(cli.action, CliAction::ConfigCompact);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_backend_command() {
    let cli = Cli::parse(
        ["config", "backend", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config backend should parse");
    assert_eq!(cli.action, CliAction::ConfigBackend);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_channels_command() {
    let cli = Cli::parse(
        ["config", "channels", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config channels should parse");
    assert_eq!(cli.action, CliAction::ConfigChannels);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_http_command() {
    let cli = Cli::parse(
        [
            "config",
            "http",
            "--config",
            "/tmp/demo.toml",
            "--no-dashboard",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config http should parse");
    assert_eq!(cli.action, CliAction::ConfigHttp);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    assert!(cli.no_dashboard);
}

#[test]
fn parses_config_trace_command() {
    let cli = Cli::parse(
        ["config", "trace", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config trace should parse");
    assert_eq!(cli.action, CliAction::ConfigTrace);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_vault_command() {
    let cli = Cli::parse(
        ["config", "vault", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config vault should parse");
    assert_eq!(cli.action, CliAction::ConfigVault);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_secret_commands_without_secret_arguments() {
    let set = Cli::parse(
        [
            "config",
            "set-secret",
            "--config",
            "/tmp/demo.toml",
            "--environment",
            "OPENAI_API_KEY",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("config set-secret should parse");
    assert_eq!(set.action, CliAction::ConfigSetSecret);
    assert_eq!(set.environment.as_deref(), Some("OPENAI_API_KEY"));

    let delete = Cli::parse(
        ["config", "delete-secret", "--environment", "OPENAI_API_KEY"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config delete-secret should parse");
    assert_eq!(delete.action, CliAction::ConfigDeleteSecret);
    assert_eq!(delete.environment.as_deref(), Some("OPENAI_API_KEY"));
}

#[test]
fn parses_config_cron_command() {
    let cli = Cli::parse(
        ["config", "cron", "--config", "/tmp/demo.toml"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("config cron should parse");
    assert_eq!(cli.action, CliAction::ConfigCron);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_render_cron_command() {
    let cli = Cli::parse(["config", "render-cron"].into_iter().map(str::to_string))
        .expect("config render-cron should parse");
    assert_eq!(cli.action, CliAction::ConfigRenderCron);
}

#[test]
fn parses_memory_queue_action() {
    let cli = Cli::parse(
        [
            "config",
            "memory-queue",
            "retry-failed",
            "--config",
            "/tmp/demo.toml",
        ]
        .into_iter()
        .map(str::to_string),
    )
    .expect("memory queue action should parse");
    assert_eq!(cli.action, CliAction::ConfigMemoryQueue);
    assert_eq!(
        cli.memory_queue_action,
        Some(crate::memory_management::MemoryQueueAction::RetryFailed)
    );
}

#[test]
fn parses_protocol_schema_command() {
    let cli = Cli::parse(["protocol", "schema"].into_iter().map(str::to_string))
        .expect("protocol schema should parse");

    assert_eq!(cli.action, CliAction::ProtocolSchema);
}

#[test]
fn rejects_unknown_config_command() {
    let error = Cli::parse(["config", "unknown"].into_iter().map(str::to_string))
        .expect_err("unknown config command should fail");
    assert_eq!(
        error.to_string(),
        "unknown config command; run `xiaoo-daemon --help` for the current command list"
    );
}

#[test]
fn parses_daemon_mcp_config_argument() {
    let cli = Cli::parse(
        ["--mcp-config", "/tmp/mcp.json"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("daemon should accept --mcp-config");

    assert_eq!(cli.mcp_config, Some(PathBuf::from("/tmp/mcp.json")));
}

#[test]
fn daemon_defaults_to_port_18080() {
    let cli = Cli::parse(std::iter::empty::<String>()).expect("cli should parse with defaults");

    assert_eq!(cli.host, "0.0.0.0");
    assert_eq!(cli.port, 18080);
}

#[test]
fn daemon_help_flag_does_not_require_config() {
    let cli =
        Cli::parse(["--help"].into_iter().map(str::to_string)).expect("cli should parse help");

    assert!(cli.help);
}

#[test]
fn dashboard_arguments_are_optional_and_default_to_none() {
    let cli = Cli::parse(std::iter::empty::<String>()).expect("cli should parse with defaults");

    assert!(cli.dashboard_host.is_none());
    assert!(cli.dashboard_port.is_none());
}

#[test]
fn parses_dashboard_host_and_port() {
    let cli = Cli::parse(
        ["--dashboard-host", "0.0.0.0", "--dashboard-port", "29000"]
            .into_iter()
            .map(str::to_string),
    )
    .expect("cli should parse dashboard flags");

    assert_eq!(cli.dashboard_host.as_deref(), Some("0.0.0.0"));
    assert_eq!(cli.dashboard_port, Some(29000));
}

#[test]
fn dashboard_port_must_be_numeric() {
    let result = Cli::parse(
        ["--dashboard-port", "not-a-number"]
            .into_iter()
            .map(str::to_string),
    );
    assert!(result.is_err());
}
