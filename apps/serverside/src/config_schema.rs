use serde_json::{json, Value};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;

pub fn config_schema() -> Value {
    json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "config_format": "toml",
        "sections": [
            section("llm", "模型", "object", &[
                field("active_profile", "string", false),
            ]),
            section("llm.profiles.*", "模型 Profile", "map", &[
                field_default("enabled", "boolean", false, json!(true)),
                field("provider", "string", true),
                field("model", "string", true),
                field("api_base", "url", false),
                secret_field("api_key_env"),
                field("context_window", "integer", false),
                field("max_tokens", "integer", false),
                enum_field("reasoning_effort", &["off", "high", "max"]),
                field_default("kvcache_enabled", "boolean", false, json!(false)),
                field_default("kvcache_debug_enabled", "boolean", false, json!(false)),
            ]),
            section("channels", "渠道", "object", &[
                field_default("interaction_timeout_secs", "integer", false, json!(600)),
            ]),
            section("channels.feishu", "飞书", "object", &[
                field_default("enabled", "boolean", false, json!(false)),
                enum_field("transport", &["webhook", "websocket"]),
                field("channel_instance_id", "string", false),
                field("app_id", "string", false),
                secret_field("app_secret_env"),
                field("verification_token", "secret", false),
                field("base_url", "url", false),
            ]),
            section("channels.telegram", "Telegram", "object", &[
                field_default("enabled", "boolean", false, json!(false)),
                field("channel_instance_id", "string", false),
                enum_field("transport", &["webhook", "polling"]),
                secret_field("bot_token_env"),
                field("webhook_secret_token", "secret", false),
                field("bot_username", "string", false),
                field("base_url", "url", false),
                field("polling_timeout_secs", "integer", false),
                field("polling_limit", "integer", false),
            ]),
            section("http", "HTTP", "object", &[
                field("bearer_token", "secret", false),
                secret_field("bearer_token_env"),
            ]),
            section("http.rate_limit", "HTTP 限流", "object", &[
                field_default("enabled", "boolean", false, json!(true)),
                field_default("requests_per_second", "integer", false, json!(2)),
                field_default("burst", "integer", false, json!(10)),
            ]),
            section("http.rate_limit.routes.*", "路由限流", "map", &[
                field_default("requests_per_second", "integer", false, json!(2)),
                field_default("burst", "integer", false, json!(10)),
            ]),
            section("http.dashboard", "Dashboard", "object", &[
                field_default("enabled", "boolean", false, json!(true)),
                field_default("host", "string", false, json!("127.0.0.1")),
                field_default("port", "integer", false, json!(28081)),
            ]),
            section("agents", "Agent 实例", "object", &[
                field("default_agent_id", "string", false),
            ]),
            section("agents.list[]", "Agent 实例项", "array", &[
                field("id", "string", true),
                field_default("default", "boolean", false, json!(false)),
                field("workspace", "path", false),
                field("profile_id", "string", false),
                field("system_prompt", "multiline", false),
            ]),
            section("agent.*", "Agent 角色", "map", &[
                field("description", "string", false),
                field("prompt", "multiline", false),
                field("max_turns", "integer", false),
                field("tools", "boolean_map", false),
            ]),
            section("subagent.*", "Subagent 角色", "map", &[
                field("description", "string", false),
                field("prompt", "multiline", false),
                field("max_turns", "integer", false),
                field("tools", "boolean_map", false),
            ]),
            section("skills", "Skills", "object", &[
                field("dirs", "path_array", false),
                field("allow_scripts", "boolean", false),
                field("disabled", "string_array", false),
            ]),
            section("hooker", "Hooks", "object", &[
                enum_field("default", &["all", "none"]),
                field("enabled", "string_array", false),
                field("disabled", "string_array", false),
                field("policies", "json_map", false),
                field("plugins", "path_array", false),
                field_default("max_prompt_chain_depth", "integer", false, json!(128)),
            ]),
            section("mcp.servers[]", "MCP Client", "array", &[
                field("name", "string", true),
                enum_field("transport", &["stdio", "sse", "streamable_http"]),
                field("command", "string", false),
                field("args", "string_array", false),
                field("env", "string_map", false),
                field("url", "url", false),
                secret_field("bearer_token_env"),
                field("agent_id", "string", false),
                field("headers", "string_map", false),
                field_default("enabled", "boolean", false, json!(true)),
                field_default("timeout_ms", "integer", false, json!(30000)),
                field("effect", "effect", false),
            ]),
            section("mcp_server", "MCP Server", "object", &[
                field_default("enabled", "boolean", false, json!(false)),
                field_default("idle_timeout_secs", "integer", false, json!(600)),
                field_default("reaper_interval_secs", "integer", false, json!(30)),
                field("allowed_origins", "string_array", false),
            ]),
            section("mcp_server.chatbot", "MCP Chatbot", "object", &[
                secret_field("bearer_token_env"),
                field("workspace", "path", false),
            ]),
            section("mcp_server.agent", "MCP Agent", "object", &[
                secret_field("bearer_token_env"),
                field("agent_role", "string", false),
            ]),
            section("lsp", "LSP", "object", &[
                field_default("enabled", "boolean", false, json!(false)),
                field("disabled_servers", "string_array", false),
            ]),
            section("lsp.extra_servers[]", "自定义 LSP", "array", &[
                field("id", "string", true),
                field("extensions", "string_array", true),
                field("command", "string", true),
                field("args", "string_array", false),
                field("root_markers", "string_array", true),
                field("language_id", "string", true),
            ]),
            section("compact", "上下文压缩", "object", &[
                field("warning_ratio", "number", false),
                field("auto_compact_ratio", "number", false),
                field("blocking_ratio", "number", false),
                field("snip_stale_after_ms", "integer", false),
                field("snip_preserve_tail", "integer", false),
                field("collapse_preserve_tail", "integer", false),
                field("summary_max_tokens", "integer", false),
                field("summary_preserve_tail", "integer", false),
                field("summary_llm_max_tokens", "integer", false),
            ]),
            section("memory_automation", "长期记忆", "object", &[
                field_default("enabled", "boolean", false, json!(false)),
                field("server", "string", false),
                field_default("recall_top_k", "integer", false, json!(5)),
                field_default("recall_token_budget", "integer", false, json!(512)),
                field_default("context_messages", "integer", false, json!(0)),
                field_default("queue_path", "path", false, json!("memory-automation-queue.jsonl")),
                field_default("queue_capacity", "integer", false, json!(256)),
                field_default("max_retries", "integer", false, json!(5)),
                field_default("retry_backoff_ms", "integer", false, json!(250)),
                field("allowed_agent_roles", "string_array", false),
            ]),
            section("server.operation_backend", "运行后端", "object", &[
                field("kind", "string", false),
                field("options", "json", false),
            ]),
            section("cron", "Cron", "object", &[
                field("jobs_dir", "path", false),
                field_default("max_concurrent_jobs", "integer", false, json!(3)),
                field_default("default_timeout_secs", "integer", false, json!(3600)),
            ]),
            section("trace", "追踪", "object", &[
                enum_field("storage_backend", &["moirai-sqlite", "stdout", "noop"]),
                field("db_path", "path", false),
            ]),
            section("paths", "路径", "object", &[
                field("data_dir", "path", false),
            ]),
            section("vault", "Vault", "object", &[
                field_default("enabled", "boolean", false, json!(false)),
                field_default("use_sdf", "boolean", false, json!(false)),
            ]),
            section("tui.remote", "TUI 远程", "object", &[
                field("url", "url", false),
                secret_field("bearer_token_env"),
                field_default("auto_connect", "boolean", false, json!(false)),
            ]),
            section("tui", "TUI", "object", &[
                field("agent_order", "string_array", false),
            ]),
            section("sandbox", "沙箱全局设置", "external", &[
                field_default("max_sandbox_cnt", "integer", false, json!(20)),
            ]),
            section("cron.jobs[]", "Cron 任务", "external", &[
                field("name", "string", true),
                field("description", "string", false),
                field("cron", "string", true),
                field("prompt", "multiline", true),
                field("agent_role", "string", false),
                field("timeout_secs", "integer", false),
                field_default("enabled", "boolean", false, json!(true)),
                field_default("max_retries", "integer", false, json!(0)),
                field_default("retry_delay_secs", "integer", false, json!(60)),
            ]),
        ]
    })
}

fn section(path: &str, title: &str, kind: &str, fields: &[Value]) -> Value {
    json!({
        "path": path,
        "title": title,
        "kind": kind,
        "fields": fields,
        "apply": "restart_daemon",
    })
}

fn field(name: &str, field_type: &str, required: bool) -> Value {
    json!({ "name": name, "type": field_type, "required": required })
}

fn field_default(name: &str, field_type: &str, required: bool, default: Value) -> Value {
    json!({
        "name": name,
        "type": field_type,
        "required": required,
        "default": default,
    })
}

fn enum_field(name: &str, values: &[&str]) -> Value {
    json!({
        "name": name,
        "type": "enum",
        "required": false,
        "values": values,
    })
}

fn secret_field(name: &str) -> Value {
    json!({
        "name": name,
        "type": "secret_env",
        "required": false,
        "secret": true,
    })
}

#[cfg(test)]
mod tests {
    use super::config_schema;

    #[test]
    fn schema_covers_primary_configuration_domains() {
        let schema = config_schema();
        let sections = schema["sections"]
            .as_array()
            .expect("sections should be an array");
        let paths = sections
            .iter()
            .filter_map(|section| section["path"].as_str())
            .collect::<Vec<_>>();

        for required in [
            "llm.profiles.*",
            "agent.*",
            "subagent.*",
            "skills",
            "hooker",
            "mcp.servers[]",
            "mcp_server",
            "lsp",
            "compact",
            "memory_automation",
            "server.operation_backend",
            "cron",
            "channels.feishu",
            "channels.telegram",
            "trace",
            "vault",
        ] {
            assert!(
                paths.contains(&required),
                "missing schema section {required}"
            );
        }
    }
}
