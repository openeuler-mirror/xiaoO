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

/// A TUI rewrite must not strip the daemon-owned `[llm]` keys.
#[test]
fn save_preserves_daemon_owned_llm_section_keys() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
provider = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"
api_base = "https://api.openai.com/v1"
max_tokens = 16384
active_profile = "qwen"
context_window = 131072

[llm.rate_limit]
rpm = 60

[llm.profiles.qwen]
enabled = true
provider = "openai-compatible"
model = "qwen3.7-plus"
api_base = "https://example.com/compatible-mode/v1"
api_key_env = "QWEN_API_KEY"
"#,
    )
    .expect("write config");

    let mut config = Config::load_from(&path).expect("load config");
    config.llm.model = "gpt-4.1".to_string();
    config.save_to(&path).expect("save config");

    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).expect("read persisted config"))
            .expect("parse persisted config");
    assert_eq!(
        persisted["llm"]["context_window"],
        toml::Value::Integer(131072)
    );
    assert_eq!(
        persisted["llm"]["rate_limit"]["rpm"],
        toml::Value::Integer(60)
    );
    assert_eq!(
        persisted["llm"]["profiles"]["qwen"]["model"],
        toml::Value::String("qwen3.7-plus".to_string())
    );
    assert_eq!(
        persisted["llm"]["profiles"]["qwen"]["api_key_env"],
        toml::Value::String("QWEN_API_KEY".to_string())
    );
    assert_eq!(
        persisted["llm"]["active_profile"],
        toml::Value::String("qwen".to_string())
    );
    assert_eq!(
        persisted["llm"]["model"],
        toml::Value::String("gpt-4.1".to_string())
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
    config.agents.default_agent_id = Some("missing".to_string());
    config.agents.list.push(super::AgentConfig {
        id: "main".to_string(),
        ..super::AgentConfig::default()
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

/// Regression for the TUI/daemon root-agent resolution fork: when the
/// `agents.default_agent_id` key is ABSENT (the daemon's standard "unset"
/// spelling) and a non-first `[[agents.list]]` entry sets `default = true`,
/// the TUI must resolve the same root agent id as the daemon (the
/// `default = true` entry, not the first entry) — see
/// `DaemonConfig::resolve_agent` in `apps/serverside/src/daemon_config.rs`.
#[test]
fn resolves_default_agent_from_default_true_entry() {
    let mut config = valid_config();
    config.agents.default_agent_id = None;
    config.agents.list = vec![
        super::AgentConfig {
            id: "main".to_string(),
            ..super::AgentConfig::default()
        },
        super::AgentConfig {
            id: "Review".to_string(),
            default: true,
            ..super::AgentConfig::default()
        },
    ];

    assert_eq!(config.resolve_default_agent_id(), "review");
    // Startup validation must follow the same chain (the `default = true`
    // entry must be accepted as the resolved default).
    assert!(config.validate_default_agent_id().is_ok());
}

#[test]
fn explicit_default_agent_id_wins_over_default_true_entry() {
    let mut config = valid_config();
    config.agents.default_agent_id = Some("main".to_string());
    config.agents.list = vec![
        super::AgentConfig {
            id: "main".to_string(),
            ..super::AgentConfig::default()
        },
        super::AgentConfig {
            id: "review".to_string(),
            default: true,
            ..super::AgentConfig::default()
        },
    ];

    assert_eq!(config.resolve_default_agent_id(), "main");
}

/// A blank `default_agent_id = ""` is treated as unset (walk the chain):
/// this is the TUI's historical local-mode behavior, and the daemon
/// rejects that spelling at startup so it cannot fork remote mode.
#[test]
fn blank_default_agent_id_treated_as_unset() {
    let mut config = valid_config();
    config.agents.default_agent_id = Some(String::new());
    config.agents.list = vec![
        super::AgentConfig {
            id: "main".to_string(),
            ..super::AgentConfig::default()
        },
        super::AgentConfig {
            id: "review".to_string(),
            default: true,
            ..super::AgentConfig::default()
        },
    ];

    assert_eq!(config.resolve_default_agent_id(), "review");
}

#[test]
fn first_entry_fallback_when_no_default_true() {
    let mut config = valid_config();
    config.agents.default_agent_id = None;
    config.agents.list = vec![
        super::AgentConfig {
            id: "main".to_string(),
            ..super::AgentConfig::default()
        },
        super::AgentConfig {
            id: "review".to_string(),
            ..super::AgentConfig::default()
        },
    ];

    assert_eq!(config.resolve_default_agent_id(), "main");
}

/// The TUI and daemon share one config.toml; a TUI rewrite (e.g. `/connect`
/// provider switch → `save_to`) must not strip the daemon-owned
/// `[[agents.list]]` fields (`default` / `workspace` / `profile_id` /
/// `system_prompt`) or materialize the `default_agent_id` key, or the
/// daemon's startup validation / root-agent resolution break on reload.
///
/// The input uses the daemon's standard "unset" spelling: the
/// `default_agent_id` key is ABSENT (a blank `default_agent_id = ""` is
/// rejected by the daemon's `validate_agents` at startup, so it is not a
/// reachable shared-config spelling).
#[test]
fn save_preserves_daemon_owned_agents_list_fields() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
provider = "openai"
model = "gpt-4o"

[[agents.list]]
id = "main"

[[agents.list]]
id = "review"
default = true
workspace = "/srv/review"
profile_id = "fast"
system_prompt = "You are a reviewer."
"#,
    )
    .expect("write config");

    let mut config = Config::load_from(&path).expect("load config");
    assert_eq!(config.resolve_default_agent_id(), "review");
    config.llm.model = "gpt-4.1".to_string();
    config.save_to(&path).expect("save config");

    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).expect("read persisted config"))
            .expect("parse persisted config");
    // The unset spelling must not be materialized as default_agent_id =
    // "main" — that either flips the daemon's root agent (non-empty list)
    // or fails its startup validation (empty list).
    assert!(persisted["agents"].get("default_agent_id").is_none());
    let list = persisted["agents"]["list"]
        .as_array()
        .expect("agents.list array");
    assert_eq!(list.len(), 2);
    // Id-only entry round-trips without gaining extra keys.
    assert_eq!(list[0]["id"].as_str(), Some("main"));
    assert!(list[0].get("default").is_none());
    // Daemon-owned fields survive the rewrite.
    assert_eq!(list[1]["default"].as_bool(), Some(true));
    assert_eq!(list[1]["workspace"].as_str(), Some("/srv/review"));
    assert_eq!(list[1]["profile_id"].as_str(), Some("fast"));
    assert_eq!(
        list[1]["system_prompt"].as_str(),
        Some("You are a reviewer.")
    );
}

/// A config without an `[agents]` section must stay without one after a
/// TUI rewrite: materializing `[agents] default_agent_id = "main"` with an
/// empty list makes the daemon bail at startup ("does not exist").
#[test]
fn save_does_not_materialize_agents_section() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
provider = "openai"
model = "gpt-4o"
"#,
    )
    .expect("write config");

    let mut config = Config::load_from(&path).expect("load config");
    config.llm.model = "gpt-4.1".to_string();
    config.save_to(&path).expect("save config");

    let persisted: toml::Value =
        toml::from_str(&std::fs::read_to_string(&path).expect("read persisted config"))
            .expect("parse persisted config");
    match persisted.get("agents") {
        None => {}
        // An empty `[agents]` header is semantically identical to absent on
        // both sides; anything beyond that is a materialization bug.
        Some(agents) => {
            assert!(agents.get("default_agent_id").is_none());
            assert!(agents.get("list").is_none());
        }
    }
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
    assert!(!TuiConfig::default().redact_secrets_display);
    assert!(!Config::default().tui.redact_secrets_display);
}

#[test]
fn checkpoint_socket_config_round_trips() {
    let config: Config = toml::from_str(
        r#"
[checkpoint]
enabled = true
transport = "unix_socket"
save_mode = "current"
[checkpoint.unix_socket]
endpoint = "/tmp/xiaoo-checkpoint.sock"
"#,
    )
    .unwrap();
    assert!(config.checkpoint.enabled);
    assert_eq!(
        config.checkpoint.save_mode,
        xiaoo_api::runtime::checkpoint::SaveMode::Current
    );
    assert_eq!(
        Config::default().checkpoint.save_mode,
        xiaoo_api::runtime::checkpoint::SaveMode::Next
    );
    assert_eq!(
        config.checkpoint.unix_socket.as_ref().unwrap().endpoint,
        std::path::PathBuf::from("/tmp/xiaoo-checkpoint.sock")
    );
    let serialized = toml::to_string(&config).unwrap();
    let restored: Config = toml::from_str(&serialized).unwrap();
    assert_eq!(restored.checkpoint, config.checkpoint);
    assert!(!Config::default().checkpoint.enabled);
    assert!(serialized.contains("transport = \"unix_socket\""));
    assert!(serialized.contains("save_mode = \"current\""));
    for old_mode in ["next_boundary", "current_boundary"] {
        assert!(
            toml::from_str::<Config>(&format!("[checkpoint]\nsave_mode = '{old_mode}'")).is_err()
        );
    }
    assert!(toml::from_str::<Config>("[checkpoint]\ntransport = 'invalid'").is_err());
}

#[test]
fn checkpoint_storage_tables_are_optional_and_round_trip() {
    use xiaoo_api::runtime::checkpoint::{StorageConfig, StorageKind};
    for source in [
        "",
        "[checkpoint.storage]",
        "[checkpoint.storage.memory]",
        "[checkpoint.storage.memory_cached_sqlite]",
    ] {
        let config: Config = toml::from_str(source).unwrap();
        assert_eq!(config.checkpoint.storage.kind, StorageKind::Memory);
        assert!(config
            .checkpoint
            .storage
            .memory
            .as_ref()
            .is_none_or(|memory| memory.capacity.is_none()));
        assert!(config
            .checkpoint
            .storage
            .memory_cached_sqlite
            .as_ref()
            .is_none_or(|sqlite| sqlite.path.is_none()
                && sqlite.compress.is_none()
                && sqlite.memory_cache_size.is_none()));
    }
    let config: Config = toml::from_str(
        r#"
[checkpoint.storage]
kind = "memory_cached_sqlite"
[checkpoint.storage.memory_cached_sqlite]
path = "xxx"
compress = "xxx"
memory_cache_size = 5
[checkpoint.storage.memory]
capacity = 50
"#,
    )
    .unwrap();
    let storage = &config.checkpoint.storage;
    assert_eq!(storage.kind, StorageKind::MemoryCachedSqlite);
    assert_eq!(storage.memory.as_ref().unwrap().capacity, Some(50));
    let sqlite = storage.memory_cached_sqlite.as_ref().unwrap();
    assert_eq!(sqlite.path.as_deref(), Some(std::path::Path::new("xxx")));
    assert_eq!(sqlite.compress.as_deref(), Some("xxx"));
    assert_eq!(sqlite.memory_cache_size, Some(5));
    let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert_eq!(restored.checkpoint.storage, *storage);
    assert_eq!(
        Config::default().checkpoint.storage,
        StorageConfig::default()
    );
}

// --- `[[agents.list]].workspace` mirror + e2b detection (#96) -------------

#[test]
fn agent_config_workspace_survives_config_round_trip() {
    let config: Config = toml::from_str(
        r#"
[[agents.list]]
id = "main"
workspace = "/tmp/pinned"
"#,
    )
    .unwrap();
    assert!(config.agent_workspace_is_pinned("main"));
    assert!(config.agents.list[0].workspace.is_some());

    // The TUI rewrites config.toml on provider switches; the mirrored
    // workspace key must survive the round trip.
    let restored: Config = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert!(restored.agent_workspace_is_pinned("main"));
}

#[test]
fn daemon_backend_kind_is_read_from_preserved_server_section() {
    let e2b: Config = toml::from_str(
        r#"
[server.operation_backend]
kind = "e2b"
"#,
    )
    .unwrap();
    assert!(e2b.daemon_uses_e2b_backend());

    let local: Config = toml::from_str(
        r#"
[server.operation_backend]
kind = "local"
"#,
    )
    .unwrap();
    assert!(!local.daemon_uses_e2b_backend());

    assert!(!Config::default().daemon_uses_e2b_backend());
    assert!(!Config::default().agent_workspace_is_pinned("main"));
}

// ── [[llm.models]] array form ────────────────────────────────────────────────

const MODELS_FORM_CONFIG: &str = r#"
[[llm.models]]
provider = "zai"
model = "glm-5"
api_base = "https://open.bigmodel.cn/api/paas/v4"
api_key_env = "ZHIPU_API_KEY"

[[llm.models]]
provider = "deepseek"
model = "deepseek-chat"
api_key_env = "DEEPSEEK_API_KEY"

[llm.models.request_header]
X-Gateway-Auth = "alpha"
"#;

fn load_tui_config(content: &str) -> Config {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(&path, content).expect("write config");
    Config::load_from(&path).expect("load config")
}

/// Load-time normalization: entry 0 fills the flat positions while the
/// array stays.
#[test]
fn load_normalizes_first_models_entry_into_flat_fields() {
    let config = load_tui_config(MODELS_FORM_CONFIG);

    assert_eq!(config.llm.provider, "zai");
    assert_eq!(config.llm.model, "glm-5");
    assert_eq!(config.llm.api_base, "https://open.bigmodel.cn/api/paas/v4");
    assert_eq!(config.llm.api_key_env.as_deref(), Some("ZHIPU_API_KEY"));
    // The entry-level table attaches to the most recent entry (deepseek).
    assert!(config.llm.request_headers.is_empty());
    assert_eq!(config.llm.models.len(), 2);
    assert_eq!(config.llm.models[1].provider, "deepseek");
    assert!(config.llm.models[0].request_headers.is_empty());
    assert_eq!(
        config.llm.models[1]
            .request_headers
            .request_header
            .as_ref()
            .and_then(|map| map.get("X-Gateway-Auth").map(String::as_str)),
        Some("alpha")
    );
}

/// A mix of array form and flat connection keys is rejected at load.
#[test]
fn load_rejects_models_array_mixed_with_flat_connection_keys() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(
        &path,
        r#"
[llm]
provider = "anthropic"

[[llm.models]]
provider = "zai"
model = "glm-5"
"#,
    )
    .expect("write config");

    let error = Config::load_from(&path).expect_err("conflict must fail load");
    assert!(
        format!("{error:#}").contains("mutually exclusive"),
        "unexpected error: {error:#}"
    );
    assert!(
        format!("{error:#}").contains("llm.provider"),
        "error must name the conflicting key: {error:#}"
    );
}

/// `save_to` round-trip: the array survives and no flat connection keys
/// leak into the file.
#[test]
fn save_round_trip_preserves_models_and_omits_flat_connection_fields() {
    let temp = tempdir().expect("tempdir");
    let path = temp.path().join("config.toml");
    std::fs::write(&path, MODELS_FORM_CONFIG).expect("write config");

    let mut config = Config::load_from(&path).expect("load config");
    config.tui.agent_order = vec!["main".to_string()];
    config.save_to(&path).expect("save config");

    let persisted: toml::Value = toml::from_str(&std::fs::read_to_string(&path).expect("read"))
        .expect("parse persisted config");
    let llm = &persisted["llm"];
    assert!(llm.get("provider").is_none());
    assert!(llm.get("model").is_none());
    assert!(llm.get("api_base").is_none());
    assert!(llm.get("api_key_env").is_none());
    assert!(llm.get("request_header").is_none());
    assert!(llm.get("request_header_env").is_none());
    let models = llm["models"].as_array().expect("models array");
    assert_eq!(models.len(), 2);
    assert_eq!(models[0]["provider"].as_str(), Some("zai"));
    assert_eq!(models[0]["api_key_env"].as_str(), Some("ZHIPU_API_KEY"));
    assert_eq!(
        models[1]["request_header"]["X-Gateway-Auth"].as_str(),
        Some("alpha")
    );
    assert_eq!(
        persisted["tui"]["agent_order"]
            .as_array()
            .and_then(|order| order.first())
            .and_then(toml::Value::as_str),
        Some("main")
    );
    // The file reloads without tripping the conflict rule.
    let reloaded = Config::load_from(&path).expect("reload config");
    assert_eq!(reloaded.llm.provider, "zai");
    assert_eq!(reloaded.llm.models.len(), 2);
}

/// A normalized models form passes bootstrap; a flat+models mix fails; a
/// pure array form is normalized by bootstrap itself.
#[test]
fn bootstrap_accepts_normalized_models_and_rejects_conflicting_state() {
    let config = load_tui_config(MODELS_FORM_CONFIG);
    let bootstrapped = require_tui_bootstrap_config(Some(config), Path::new("/tmp/config.toml"))
        .expect("models form should bootstrap");
    assert_eq!(bootstrapped.llm.provider, "zai");

    let mut conflicting = valid_config();
    conflicting.llm.models = vec![xiaoo_api::llm::LlmModelEntry {
        provider: "zai".to_string(),
        model: "glm-5".to_string(),
        api_base: None,
        api_key_env: None,
        request_headers: Default::default(),
    }];
    let error = require_tui_bootstrap_config(Some(conflicting), Path::new("/tmp/config.toml"))
        .expect_err("flat + models mix must fail bootstrap");
    assert!(
        format!("{error:#}").contains("mutually exclusive"),
        "unexpected error: {error:#}"
    );

    let mut config = Config::default();
    config.llm.models = vec![xiaoo_api::llm::LlmModelEntry {
        provider: "deepseek".to_string(),
        model: "deepseek-chat".to_string(),
        api_base: None,
        api_key_env: None,
        request_headers: Default::default(),
    }];
    let bootstrapped = require_tui_bootstrap_config(Some(config), Path::new("/tmp/config.toml"))
        .expect("pure array form should bootstrap");
    assert_eq!(bootstrapped.llm.provider, "deepseek");
    assert_eq!(bootstrapped.llm.model, "deepseek-chat");
}
