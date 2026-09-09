use crate::config_schema::CONFIG_SCHEMA_VERSION;
use xiaoo_shared::daemon_protocol::response::{ManagementCapabilities, ManagementDomainCapability};

pub fn management_capabilities() -> ManagementCapabilities {
    ManagementCapabilities {
        api_version: 1,
        config_schema_version: CONFIG_SCHEMA_VERSION,
        config_commands: [
            "config schema",
            "config providers",
            "config validate",
            "config inspect",
            "config test-model",
            "config models",
            "config roles",
            "config agents",
            "config test-agent",
            "config tools",
            "config custom-tools",
            "config render-custom-tool",
            "config test-custom-tool",
            "config skills",
            "config hooks",
            "config mcp",
            "config mcp-server",
            "config lsp",
            "config memory",
            "config memory-queue",
            "config compact",
            "config backend",
            "config channels",
            "config http",
            "config trace",
            "config cron",
            "config render-cron",
        ]
        .into_iter()
        .map(str::to_string)
        .collect(),
        domains: vec![
            domain(
                "config",
                true,
                false,
                false,
                true,
                &["schema", "providers", "validate", "inspect"],
            ),
            domain(
                "models",
                true,
                false,
                true,
                true,
                &["session_select", "test_connection", "list_catalog"],
            ),
            domain(
                "agents",
                true,
                false,
                false,
                true,
                &["list", "test_startup"],
            ),
            domain("roles", true, false, false, false, &["list"]),
            domain("subagents", true, false, false, false, &["list_roles"]),
            domain("tools", true, false, false, false, &["list"]),
            domain(
                "custom_tools",
                true,
                true,
                false,
                true,
                &["list", "validate", "render", "test"],
            ),
            domain("skills", true, false, false, false, &["list"]),
            domain("hooks", true, false, false, false, &["list"]),
            domain(
                "mcp_client",
                true,
                true,
                false,
                true,
                &["list", "test_connections"],
            ),
            domain(
                "mcp_server",
                true,
                true,
                false,
                true,
                &["inspect", "preflight"],
            ),
            domain("lsp", true, true, false, true, &["list", "detect"]),
            domain(
                "memory",
                true,
                true,
                false,
                true,
                &[
                    "inspect",
                    "test_connection",
                    "queue_status",
                    "retry_failed_queue",
                    "clear_failed_queue",
                ],
            ),
            domain("compact", true, true, false, true, &["inspect", "validate"]),
            domain(
                "backend",
                true,
                true,
                false,
                true,
                &["inspect", "preflight"],
            ),
            domain(
                "cron",
                true,
                true,
                true,
                true,
                &["list", "validate", "render", "runtime_status", "run_now"],
            ),
            domain(
                "channels",
                true,
                true,
                false,
                true,
                &["inspect", "preflight", "runtime_status", "test_connection"],
            ),
            domain("http", true, true, false, true, &["inspect", "preflight"]),
            domain(
                "trace",
                true,
                true,
                false,
                true,
                &["inspect", "diagnose", "list_recent"],
            ),
            domain("vault", true, false, false, false, &[]),
            domain(
                "sessions",
                false,
                false,
                true,
                false,
                &["open", "input", "cancel", "close", "heartbeat", "detach"],
            ),
            domain(
                "checkpoints",
                false,
                true,
                true,
                false,
                &["create", "delete_snapshot", "checkout", "export"],
            ),
            domain(
                "sandboxes",
                true,
                true,
                true,
                false,
                &["list", "pause", "resume", "exec", "read_file", "write_file"],
            ),
        ],
    }
}

fn domain(
    id: &'static str,
    configurable: bool,
    runtime_read: bool,
    runtime_write: bool,
    test: bool,
    actions: &[&'static str],
) -> ManagementDomainCapability {
    ManagementDomainCapability {
        id: id.to_string(),
        configurable,
        runtime_read,
        runtime_write,
        test,
        actions: actions.iter().map(|action| (*action).to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::management_capabilities;

    #[test]
    fn reports_supported_and_unimplemented_management_actions_honestly() {
        let capabilities = management_capabilities();
        let models = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "models")
            .expect("models capability");
        assert!(models.configurable);
        assert!(models.test);
        assert!(models
            .actions
            .iter()
            .any(|action| action == "session_select"));
        assert!(models
            .actions
            .iter()
            .any(|action| action == "test_connection"));
        assert!(models.actions.iter().any(|action| action == "list_catalog"));

        let roles = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "roles")
            .expect("roles capability");
        assert!(roles.actions.iter().any(|action| action == "list"));

        let agents = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "agents")
            .expect("agents capability");
        assert!(agents.test);
        assert!(agents.actions.iter().any(|action| action == "list"));
        assert!(agents.actions.iter().any(|action| action == "test_startup"));

        let tools = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "tools")
            .expect("tools capability");
        assert!(tools.actions.iter().any(|action| action == "list"));

        let skills = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "skills")
            .expect("skills capability");
        assert!(skills.configurable);
        assert!(!skills.runtime_read);
        assert!(!skills.runtime_write);
        assert!(!skills.test);
        assert!(skills.actions.iter().any(|action| action == "list"));

        let custom_tools = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "custom_tools")
            .expect("custom tools capability");
        assert!(custom_tools.runtime_read);
        assert!(custom_tools.test);
        assert!(custom_tools.actions.iter().any(|action| action == "list"));
        assert!(custom_tools
            .actions
            .iter()
            .any(|action| action == "validate"));
        assert!(custom_tools.actions.iter().any(|action| action == "render"));
        assert!(custom_tools.actions.iter().any(|action| action == "test"));

        let hooks = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "hooks")
            .expect("hooks capability");
        assert!(hooks.configurable);
        assert!(hooks.actions.iter().any(|action| action == "list"));

        let mcp = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "mcp_client")
            .expect("MCP capability");
        assert!(mcp.runtime_read);
        assert!(mcp.test);
        assert!(mcp.actions.iter().any(|action| action == "list"));
        assert!(mcp
            .actions
            .iter()
            .any(|action| action == "test_connections"));

        let lsp = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "lsp")
            .expect("LSP capability");
        assert!(lsp.runtime_read);
        assert!(lsp.test);
        assert!(lsp.actions.iter().any(|action| action == "list"));
        assert!(lsp.actions.iter().any(|action| action == "detect"));

        let mcp_server = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "mcp_server")
            .expect("MCP Server capability");
        assert!(mcp_server.runtime_read);
        assert!(mcp_server.test);
        assert!(mcp_server.actions.iter().any(|action| action == "inspect"));
        assert!(mcp_server
            .actions
            .iter()
            .any(|action| action == "preflight"));

        let sandboxes = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "sandboxes")
            .expect("sandbox capability");
        assert!(sandboxes.runtime_read);
        assert!(sandboxes.actions.iter().any(|action| action == "list"));

        let memory = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "memory")
            .expect("memory capability");
        assert!(memory.runtime_read);
        assert!(memory.test);
        assert!(memory.actions.iter().any(|action| action == "inspect"));
        assert!(memory
            .actions
            .iter()
            .any(|action| action == "test_connection"));
        assert!(memory.actions.iter().any(|action| action == "queue_status"));
        assert!(memory
            .actions
            .iter()
            .any(|action| action == "retry_failed_queue"));
        assert!(memory
            .actions
            .iter()
            .any(|action| action == "clear_failed_queue"));

        let compact = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "compact")
            .expect("compact capability");
        assert!(compact.runtime_read);
        assert!(compact.test);
        assert!(compact.actions.iter().any(|action| action == "inspect"));
        assert!(compact.actions.iter().any(|action| action == "validate"));

        let backend = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "backend")
            .expect("backend capability");
        assert!(backend.runtime_read);
        assert!(backend.test);
        assert!(backend.actions.iter().any(|action| action == "preflight"));

        let channels = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "channels")
            .expect("channel capability");
        assert!(channels.runtime_read);
        assert!(channels.test);
        assert!(channels.actions.iter().any(|action| action == "inspect"));
        assert!(channels.actions.iter().any(|action| action == "preflight"));
        assert!(channels
            .actions
            .iter()
            .any(|action| action == "runtime_status"));
        assert!(channels
            .actions
            .iter()
            .any(|action| action == "test_connection"));

        let http = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "http")
            .expect("HTTP capability");
        assert!(http.runtime_read);
        assert!(http.test);
        assert!(http.actions.iter().any(|action| action == "inspect"));
        assert!(http.actions.iter().any(|action| action == "preflight"));

        let trace = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "trace")
            .expect("Trace capability");
        assert!(trace.runtime_read);
        assert!(trace.test);
        assert!(trace.actions.iter().any(|action| action == "inspect"));
        assert!(trace.actions.iter().any(|action| action == "list_recent"));

        let cron = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "cron")
            .expect("Cron capability");
        assert!(cron.runtime_read);
        assert!(cron.runtime_write);
        assert!(cron.actions.iter().any(|action| action == "runtime_status"));
        assert!(cron.actions.iter().any(|action| action == "run_now"));
    }
}
