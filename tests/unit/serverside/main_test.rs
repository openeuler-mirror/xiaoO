use super::{ensure_external_bind_is_authenticated, Cli, CliAction};
use std::net::SocketAddr;
use std::path::PathBuf;

fn parse_cli(args: &[&str]) -> Cli {
    Cli::parse(args.iter().map(|arg| arg.to_string())).expect("cli should parse")
}

#[test]
fn parses_daemon_arguments() {
    let cli = parse_cli(&[
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
    ]);

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

/// Every `config <sub> --config <path>` variant shares the same parse shape:
/// the subcommand selects the action and `--config` lands in `cli.config`.
#[test]
fn parses_config_commands_with_config_path() {
    let cases: &[(&[&str], CliAction)] = &[
        (
            &["config", "validate", "--config", "/tmp/demo.toml"],
            CliAction::ValidateConfig,
        ),
        (
            &["config", "roles", "--config", "/tmp/demo.toml"],
            CliAction::ConfigRoles,
        ),
        (
            &["config", "agents", "--config", "/tmp/demo.toml"],
            CliAction::ConfigAgents,
        ),
        (
            &["config", "tools", "--config", "/tmp/demo.toml"],
            CliAction::ConfigTools,
        ),
        (
            &["config", "custom-tools", "--config", "/tmp/demo.toml"],
            CliAction::ConfigCustomTools,
        ),
        (
            &["config", "skills", "--config", "/tmp/demo.toml"],
            CliAction::ConfigSkills,
        ),
        (
            &["config", "hooks", "--config", "/tmp/demo.toml"],
            CliAction::ConfigHooks,
        ),
        (
            &["config", "lsp", "--config", "/tmp/demo.toml"],
            CliAction::ConfigLsp,
        ),
        (
            &["config", "mcp-server", "--config", "/tmp/demo.toml"],
            CliAction::ConfigMcpServer,
        ),
        (
            &["config", "memory", "--config", "/tmp/demo.toml"],
            CliAction::ConfigMemory,
        ),
        (
            &["config", "compact", "--config", "/tmp/demo.toml"],
            CliAction::ConfigCompact,
        ),
        (
            &["config", "backend", "--config", "/tmp/demo.toml"],
            CliAction::ConfigBackend,
        ),
        (
            &["config", "channels", "--config", "/tmp/demo.toml"],
            CliAction::ConfigChannels,
        ),
        (
            &["config", "trace", "--config", "/tmp/demo.toml"],
            CliAction::ConfigTrace,
        ),
        (
            &["config", "vault", "--config", "/tmp/demo.toml"],
            CliAction::ConfigVault,
        ),
        (
            &["config", "cron", "--config", "/tmp/demo.toml"],
            CliAction::ConfigCron,
        ),
    ];
    for (args, action) in cases {
        let cli = parse_cli(args);
        assert_eq!(cli.action, *action, "{args:?}");
        assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    }
}

/// Flag-less `config` subcommands only map onto their action.
#[test]
fn parses_config_commands_without_extra_arguments() {
    let cases: &[(&[&str], CliAction)] = &[
        (&["config", "schema"], CliAction::ConfigSchema),
        (&["config", "providers"], CliAction::ConfigProviders),
        (
            &["config", "render-custom-tool"],
            CliAction::ConfigRenderCustomTool,
        ),
        (&["config", "render-cron"], CliAction::ConfigRenderCron),
    ];
    for (args, action) in cases {
        assert_eq!(parse_cli(args).action, *action, "{args:?}");
    }
}

#[test]
fn parses_config_inspect_with_startup_overrides() {
    let cli = parse_cli(&[
        "config",
        "inspect",
        "--config",
        "/tmp/demo.toml",
        "--no-dashboard",
        "--host",
        "127.0.0.1",
    ]);

    assert_eq!(cli.action, CliAction::ConfigInspect);
    assert!(cli.no_dashboard);
    assert_eq!(cli.host, "127.0.0.1");
}

#[test]
fn parses_config_test_model_command() {
    let cli = parse_cli(&[
        "config",
        "test-model",
        "--profile",
        "qwen",
        "--config",
        "/tmp/demo.toml",
    ]);

    assert_eq!(cli.action, CliAction::ConfigTestModel);
    assert_eq!(cli.profile.as_deref(), Some("qwen"));
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
}

#[test]
fn parses_config_models_command() {
    let cli = parse_cli(&["config", "models", "--profile", "qwen"]);

    assert_eq!(cli.action, CliAction::ConfigModels);
    assert_eq!(cli.profile.as_deref(), Some("qwen"));
}

#[test]
fn parses_config_test_agent_command() {
    let cli = parse_cli(&["config", "test-agent", "--agent", "review"]);

    assert_eq!(cli.action, CliAction::ConfigTestAgent);
    assert_eq!(cli.agent.as_deref(), Some("review"));
}

#[test]
fn parses_config_test_custom_tool_command() {
    let cli = parse_cli(&[
        "config",
        "test-custom-tool",
        "--manifest",
        "/tmp/echo.toml",
        "--allow-effects",
    ]);

    assert_eq!(cli.action, CliAction::ConfigTestCustomTool);
    assert_eq!(cli.manifest, Some(PathBuf::from("/tmp/echo.toml")));
    assert!(cli.allow_effects);
}

#[test]
fn parses_config_test_hook_command() {
    let cli = parse_cli(&[
        "config",
        "test-hook",
        "--hooker",
        "builtin_session_created_hooker",
        "--config",
        "/tmp/demo.toml",
    ]);

    assert_eq!(cli.action, CliAction::ConfigTestHook);
    assert_eq!(
        cli.hooker.as_deref(),
        Some("builtin_session_created_hooker")
    );
}

#[test]
fn parses_config_mcp_command() {
    let cli = parse_cli(&[
        "config",
        "mcp",
        "--config",
        "/tmp/demo.toml",
        "--mcp-config",
        "/tmp/mcp.json",
    ]);

    assert_eq!(cli.action, CliAction::ConfigMcp);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    assert_eq!(cli.mcp_config, Some(PathBuf::from("/tmp/mcp.json")));
}

#[test]
fn parses_config_lsp_action_commands() {
    let install = parse_cli(&["config", "install-lsp", "--server", "gopls"]);
    assert_eq!(install.action, CliAction::ConfigInstallLsp);
    assert_eq!(install.server.as_deref(), Some("gopls"));

    let test = parse_cli(&[
        "config",
        "test-lsp",
        "--server",
        "rust-analyzer",
        "--workspace",
        "/tmp/workspace",
    ]);
    assert_eq!(test.action, CliAction::ConfigTestLsp);
    assert_eq!(test.server.as_deref(), Some("rust-analyzer"));
    assert_eq!(test.workspace, Some(PathBuf::from("/tmp/workspace")));
}

#[test]
fn parses_config_http_command() {
    let cli = parse_cli(&[
        "config",
        "http",
        "--config",
        "/tmp/demo.toml",
        "--no-dashboard",
    ]);
    assert_eq!(cli.action, CliAction::ConfigHttp);
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/demo.toml")));
    assert!(cli.no_dashboard);
}

#[test]
fn parses_config_secret_commands_without_secret_arguments() {
    let set = parse_cli(&[
        "config",
        "set-secret",
        "--config",
        "/tmp/demo.toml",
        "--environment",
        "OPENAI_API_KEY",
    ]);
    assert_eq!(set.action, CliAction::ConfigSetSecret);
    assert_eq!(set.environment.as_deref(), Some("OPENAI_API_KEY"));

    let delete = parse_cli(&["config", "delete-secret", "--environment", "OPENAI_API_KEY"]);
    assert_eq!(delete.action, CliAction::ConfigDeleteSecret);
    assert_eq!(delete.environment.as_deref(), Some("OPENAI_API_KEY"));
}

#[test]
fn parses_memory_queue_action() {
    let cli = parse_cli(&[
        "config",
        "memory-queue",
        "retry-failed",
        "--config",
        "/tmp/demo.toml",
    ]);
    assert_eq!(cli.action, CliAction::ConfigMemoryQueue);
    assert_eq!(
        cli.memory_queue_action,
        Some(crate::memory_management::MemoryQueueAction::RetryFailed)
    );
}

#[test]
fn parses_protocol_schema_command() {
    let cli = parse_cli(&["protocol", "schema"]);

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
    let cli = parse_cli(&["--mcp-config", "/tmp/mcp.json"]);

    assert_eq!(cli.mcp_config, Some(PathBuf::from("/tmp/mcp.json")));
}

#[test]
fn daemon_defaults_to_loopback_host_and_port_18080() {
    let cli = parse_cli(&[]);

    // The runtime API exposes shell execution, so the default must not bind an
    // externally reachable interface.
    assert_eq!(cli.host, "127.0.0.1");
    assert_eq!(cli.port, 18080);
}

#[test]
fn external_bind_without_auth_is_rejected() {
    let addr: SocketAddr = "0.0.0.0:18080".parse().expect("valid socket address");

    let error = ensure_external_bind_is_authenticated(addr, false)
        .expect_err("an unauthenticated external bind must be refused");

    let message = error.to_string();
    assert!(message.contains("refusing to start"), "{message}");
    assert!(message.contains("bearer_token"), "{message}");
}

#[test]
fn external_bind_with_auth_is_allowed() {
    let addr: SocketAddr = "0.0.0.0:18080".parse().expect("valid socket address");

    assert!(ensure_external_bind_is_authenticated(addr, true).is_ok());
}

#[test]
fn loopback_bind_without_auth_is_allowed() {
    let v4: SocketAddr = "127.0.0.1:18080".parse().expect("valid socket address");
    let v6: SocketAddr = "[::1]:18080".parse().expect("valid socket address");

    assert!(ensure_external_bind_is_authenticated(v4, false).is_ok());
    assert!(ensure_external_bind_is_authenticated(v6, false).is_ok());
}

#[test]
fn daemon_help_flag_does_not_require_config() {
    let cli = parse_cli(&["--help"]);

    assert!(cli.help);
}

#[test]
fn dashboard_arguments_are_optional_and_default_to_none() {
    let cli = parse_cli(&[]);

    assert!(cli.dashboard_host.is_none());
    assert!(cli.dashboard_port.is_none());
}

#[test]
fn parses_dashboard_host_and_port() {
    let cli = parse_cli(&["--dashboard-host", "0.0.0.0", "--dashboard-port", "29000"]);

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
