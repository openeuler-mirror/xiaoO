use super::{resolve_config_path, AppConfig, DaemonConfig};
use tempfile::TempDir;

#[test]
fn loads_native_active_llm_profile() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[llm]
active_profile = "qwen"

[llm.profiles.qwen]
provider = "openai-compatible"
model = "qwen3.7-plus"
api_base = "https://example.com/v1"
api_key_env = "QWEN_API_KEY"
max_tokens = 8192
reasoning_effort = "high"
"#,
    )
    .expect("write config");

    let config = DaemonConfig::load_from(&config_path).expect("load profile config");
    assert_eq!(config.app.llm.active_profile.as_deref(), Some("qwen"));
    assert_eq!(config.app.llm.provider, "openai-compatible");
    assert_eq!(config.app.llm.model, "qwen3.7-plus");
    assert_eq!(config.app.llm.api_key_env.as_deref(), Some("QWEN_API_KEY"));
    assert_eq!(config.max_output_tokens(), 8192);
    assert_eq!(
        config.app.llm.profiles["qwen"].reasoning_effort,
        xiaoo_api::chat::ReasoningEffort::High
    );
}

#[test]
fn rejects_disabled_active_llm_profile() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    std::fs::write(
        &config_path,
        r#"
[llm]
active_profile = "disabled"

[llm.profiles.disabled]
enabled = false
provider = "openai"
model = "gpt-4o"
"#,
    )
    .expect("write config");

    let error = DaemonConfig::load_from(&config_path).expect_err("profile must be rejected");
    assert!(error.to_string().contains("invalid llm config"));
    assert!(format!("{error:#}").contains("llm profile `disabled` is disabled"));
}

#[test]
fn rejects_invalid_agent_references_and_duplicate_ids() {
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
default_agent_id = "missing"
[[agents.list]]
id = "main"
profile_id = "unknown"
[[agents.list]]
id = "main"
"#,
    )
    .expect("config");
    let error = DaemonConfig::load_from(&config_path).expect_err("invalid agents");
    assert!(error.to_string().contains("invalid agents config"));
}

#[test]
fn parses_memory_automation_config() {
    let content = r#"
[llm]
provider = "openai"
model = "gpt-4o"

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
"#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");

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
fn daemon_load_merges_runtime_json_mcp_servers() {
    let temp = TempDir::new().expect("tempdir");
    let config_path = temp.path().join("config.toml");
    let json_path = temp.path().join("mcp.json");
    std::fs::write(
        &config_path,
        r#"
[llm]
provider = "openrouter"
model = "z-ai/glm-5"

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

    let daemon = DaemonConfig::load_with_mcp_config(
        &config_path,
        Some(&json_path),
        temp.path(),
        Some(temp.path()),
    )
    .expect("load merged daemon config");

    assert_eq!(
        daemon
            .app
            .mcp
            .servers
            .iter()
            .map(|server| server.name.as_str())
            .collect::<Vec<_>>(),
        vec!["toml-server", "json-server"]
    );
}

#[test]
fn parses_feishu_channel_config() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [channels.feishu]
            enabled = true
            app_id = "cli_123"
            app_secret_env = "FEISHU_APP_SECRET"
            verification_token = "verify-token"
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };
    let feishu = daemon
        .feishu_config()
        .expect("feishu config should validate")
        .expect("feishu should be enabled");
    assert_eq!(feishu.app_id, "cli_123");
    assert_eq!(feishu.base_url, "https://open.feishu.cn");
}

#[test]
fn parses_feishu_websocket_config_without_webhook_runtime() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [channels.feishu]
            enabled = true
            transport = "websocket"
            channel_instance_id = "ops-feishu"
            app_id = "cli_123"
            app_secret_env = "FEISHU_APP_SECRET"
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };
    let feishu = daemon
        .feishu_config()
        .expect("feishu websocket config should validate")
        .expect("feishu should be enabled");

    assert_eq!(
        feishu.event_transport,
        super::FeishuEventTransport::Websocket
    );
    assert_eq!(feishu.channel_instance_id.as_deref(), Some("ops-feishu"));
    assert!(feishu.verification_token.is_none());
    assert!(daemon
        .channel_runtimes()
        .expect("webhook channel runtimes should resolve")
        .is_empty());
}

#[test]
fn parses_telegram_channel_config() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [channels.telegram]
            enabled = true
            channel_instance_id = "ops-telegram"
            bot_token_env = "TELEGRAM_BOT_TOKEN"
            webhook_secret_token = "secret_token-1"
            bot_username = "@xiaoO_bot"
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };
    let telegram = daemon
        .telegram_config()
        .expect("telegram config should validate")
        .expect("telegram should be enabled");

    assert_eq!(
        telegram.channel_instance_id.as_deref(),
        Some("ops-telegram")
    );
    assert_eq!(telegram.bot_token_env, "TELEGRAM_BOT_TOKEN");
    assert_eq!(
        telegram.event_transport,
        super::TelegramEventTransport::Webhook
    );
    assert_eq!(telegram.base_url, "https://api.telegram.org");
    assert_eq!(telegram.polling_timeout_secs, 50);
    assert_eq!(telegram.polling_limit, 100);
}

#[test]
fn parses_telegram_polling_channel_config() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [channels.telegram]
            enabled = true
            transport = "polling"
            bot_token_env = "TELEGRAM_BOT_TOKEN"
            polling_timeout_secs = 30
            polling_limit = 25
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };
    let telegram = daemon
        .telegram_config()
        .expect("telegram config should validate")
        .expect("telegram should be enabled");

    assert_eq!(
        telegram.event_transport,
        super::TelegramEventTransport::Polling
    );
    assert_eq!(telegram.polling_timeout_secs, 30);
    assert_eq!(telegram.polling_limit, 25);
    assert!(daemon
        .channel_runtimes()
        .expect("channel runtimes should resolve")
        .is_empty());
}

#[test]
fn resolves_server_operation_backend_from_server_namespace() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [server.operation_backend]
            kind = "e2b"

            [server.operation_backend.options]
            api_key = "test-key"
            template_id = "base"
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };
    let backend = daemon
        .server_operation_backend()
        .expect("server backend should resolve");

    assert_eq!(backend.kind, "e2b");
    assert_eq!(backend.options["api_key"].as_str(), Some("test-key"));
    assert_eq!(backend.options["template_id"].as_str(), Some("base"));
}

#[test]
fn daemon_ignores_top_level_operation_backend() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [operation_backend]
            kind = "e2b"

            [operation_backend.options]
            api_key = "test-key"
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };

    assert!(daemon.server_operation_backend().is_none());
}

#[test]
fn resolves_xdg_config_when_present() {
    let temp = TempDir::new().expect("tempdir");
    let xdg_dir = temp.path().join(".config/xiaoo");
    std::fs::create_dir_all(&xdg_dir).expect("create xdg dir");
    let config_path = xdg_dir.join("config.toml");
    std::fs::write(&config_path, "").expect("write config");

    let previous_home = std::env::var_os("HOME");
    std::env::set_var("HOME", temp.path());
    let resolved = resolve_config_path(None).expect("resolve path");
    if let Some(home) = previous_home {
        std::env::set_var("HOME", home);
    } else {
        std::env::remove_var("HOME");
    }

    assert_eq!(resolved, config_path);
}

#[test]
fn parses_agent_role_presets() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [agent.code-reviewer]
            description = "Reviews code for best practices and potential issues"
            prompt = "You are a code reviewer."
            max_turns = 3

            [agent.code-reviewer.tools]
            file_write = false
            file_edit = false
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let role = config
        .agent
        .get("code-reviewer")
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
fn resolves_http_bearer_token_from_env() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [http]
            bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN_TEST"
        "#;

    let previous = std::env::var_os("XIAOO_HTTP_BEARER_TOKEN_TEST");
    std::env::set_var("XIAOO_HTTP_BEARER_TOKEN_TEST", "test-token");

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };
    let token = daemon
        .http_bearer_token()
        .expect("http auth should resolve")
        .expect("token should be present");

    if let Some(value) = previous {
        std::env::set_var("XIAOO_HTTP_BEARER_TOKEN_TEST", value);
    } else {
        std::env::remove_var("XIAOO_HTTP_BEARER_TOKEN_TEST");
    }

    assert_eq!(token, "test-token");
}

#[test]
fn rejects_conflicting_http_bearer_token_sources() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [http]
            bearer_token = "inline-token"
            bearer_token_env = "XIAOO_HTTP_BEARER_TOKEN_TEST"
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app: config,
        config_path: "config.toml".into(),
    };

    let error = daemon.http_bearer_token().expect_err("config should fail");
    assert!(error
        .to_string()
        .contains("http.bearer_token and http.bearer_token_env are mutually exclusive"));
}

#[test]
fn parses_http_rate_limit_config() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [http.rate_limit]
            enabled = true
            requests_per_second = 5
            burst = 20

            [http.rate_limit.routes.health]
            requests_per_second = 10
            burst = 30
        "#;

    let config: AppConfig = toml::from_str(content).expect("config should parse");
    assert!(config.http.rate_limit.is_some());

    let rl = config.http.rate_limit.unwrap();
    assert!(rl.enabled);
    assert_eq!(rl.requests_per_second, 5);
    assert_eq!(rl.burst, 20);

    let health_override = rl.routes.get("health").expect("health route override");
    assert_eq!(health_override.requests_per_second, 10);
    assert_eq!(health_override.burst, 30);
}

#[test]
fn http_config_defaults_to_no_rate_limit() {
    use crate::daemon_config::HttpConfig;
    let config: HttpConfig = toml::from_str("").expect("empty should parse");
    assert!(config.rate_limit.is_none());
    assert!(config.bearer_token.is_none());
    assert!(config.bearer_token_env.is_none());
}

#[test]
fn resolves_enabled_mcp_server_and_creates_empty_workspace() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("chatbot-empty");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let chat_env = format!("XIAOO_TEST_MCP_CHAT_{suffix}");
    let agent_env = format!("XIAOO_TEST_MCP_AGENT_{suffix}");
    std::env::set_var(&chat_env, "chat-token");
    std::env::set_var(&agent_env, "agent-token");
    let content = format!(
        r#"
                [llm]
                provider = "openrouter"
                model = "z-ai/glm-5"

                [agent.xuanyuan]
                description = "Operations controller"
                prompt = "Delegate diagnostics to the configured subagents."

                [mcp_server]
                enabled = true
                allowed_origins = ["https://example.com"]

                [mcp_server.chatbot]
                bearer_token_env = "{chat_env}"
                workspace = "{}"

                [mcp_server.agent]
                bearer_token_env = "{agent_env}"
                agent_role = "xuanyuan"
            "#,
        workspace.display()
    );
    let app: AppConfig = toml::from_str(&content).expect("config should parse");
    let daemon = DaemonConfig {
        app,
        config_path: "config.toml".into(),
    };

    let resolved = daemon
        .resolve_mcp_server_config()
        .expect("MCP config should validate")
        .expect("MCP should be enabled");
    std::env::remove_var(&chat_env);
    std::env::remove_var(&agent_env);

    assert_eq!(resolved.idle_timeout_secs, 600);
    assert_eq!(resolved.reaper_interval_secs, 30);
    assert_eq!(resolved.chatbot_token, "chat-token");
    assert_eq!(resolved.agent_token, "agent-token");
    assert_eq!(resolved.agent_role.as_deref(), Some("xuanyuan"));
    assert_eq!(resolved.allowed_origins, vec!["https://example.com"]);
    assert_eq!(
        resolved.chatbot_workspace,
        workspace.canonicalize().expect("canonical workspace")
    );
}

#[test]
fn rejects_unknown_mcp_agent_role() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [mcp_server]
            enabled = true

            [mcp_server.agent]
            agent_role = "missing-role"
        "#;
    let app: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app,
        config_path: "config.toml".into(),
    };

    let error = daemon
        .resolve_mcp_server_config()
        .expect_err("unknown MCP agent role must be rejected");

    assert!(error
        .to_string()
        .contains("references unknown agent role `missing-role`"));
}

#[test]
fn rejects_mcp_agent_when_daemon_backend_is_not_local() {
    let content = r#"
            [llm]
            provider = "openrouter"
            model = "z-ai/glm-5"

            [mcp_server]
            enabled = true

            [server.operation_backend]
            kind = "e2b"
        "#;
    let app: AppConfig = toml::from_str(content).expect("config should parse");
    let daemon = DaemonConfig {
        app,
        config_path: "config.toml".into(),
    };
    let error = daemon
        .resolve_mcp_server_config()
        .expect_err("E2B must be rejected");
    assert!(error
        .to_string()
        .contains("requires server.operation_backend"));
}

#[test]
fn rejects_equal_mcp_tokens() {
    let temp = TempDir::new().expect("tempdir");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let chat_env = format!("XIAOO_TEST_MCP_CHAT_EQUAL_{suffix}");
    let agent_env = format!("XIAOO_TEST_MCP_AGENT_EQUAL_{suffix}");
    std::env::set_var(&chat_env, "same-token");
    std::env::set_var(&agent_env, "same-token");
    let content = format!(
        r#"
                [llm]
                provider = "openrouter"
                model = "z-ai/glm-5"

                [mcp_server]
                enabled = true

                [mcp_server.chatbot]
                bearer_token_env = "{chat_env}"
                workspace = "{}"

                [mcp_server.agent]
                bearer_token_env = "{agent_env}"
            "#,
        temp.path().join("chatbot-empty").display()
    );
    let app: AppConfig = toml::from_str(&content).expect("config should parse");
    let daemon = DaemonConfig {
        app,
        config_path: "config.toml".into(),
    };
    let error = daemon
        .resolve_mcp_server_config()
        .expect_err("equal MCP tokens must be rejected");
    std::env::remove_var(&chat_env);
    std::env::remove_var(&agent_env);

    assert!(error.to_string().contains("must be different"));
}

#[test]
fn refuses_to_reuse_nonempty_chatbot_workspace() {
    let temp = TempDir::new().expect("tempdir");
    let workspace = temp.path().join("chatbot-nonempty");
    std::fs::create_dir(&workspace).expect("create workspace");
    std::fs::write(workspace.join("keep.txt"), "do not delete").expect("seed workspace");
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let chat_env = format!("XIAOO_TEST_MCP_CHAT_NONEMPTY_{suffix}");
    let agent_env = format!("XIAOO_TEST_MCP_AGENT_NONEMPTY_{suffix}");
    std::env::set_var(&chat_env, "chat-token");
    std::env::set_var(&agent_env, "agent-token");
    let content = format!(
        r#"
                [llm]
                provider = "openrouter"
                model = "z-ai/glm-5"

                [mcp_server]
                enabled = true

                [mcp_server.chatbot]
                bearer_token_env = "{chat_env}"
                workspace = "{}"

                [mcp_server.agent]
                bearer_token_env = "{agent_env}"
            "#,
        workspace.display()
    );
    let app: AppConfig = toml::from_str(&content).expect("config should parse");
    let daemon = DaemonConfig {
        app,
        config_path: "config.toml".into(),
    };
    let error = daemon
        .resolve_mcp_server_config()
        .expect_err("nonempty chatbot workspace must be rejected");
    std::env::remove_var(&chat_env);
    std::env::remove_var(&agent_env);

    assert!(error.to_string().contains("must be empty"));
    assert!(workspace.join("keep.txt").is_file());
}
