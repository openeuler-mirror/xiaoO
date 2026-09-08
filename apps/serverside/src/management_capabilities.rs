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
            "config tools",
            "config skills",
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
            domain("agents", true, false, false, false, &[]),
            domain("roles", true, false, false, false, &["list"]),
            domain("subagents", true, false, false, false, &["list_roles"]),
            domain("tools", true, false, false, false, &["list"]),
            domain("skills", true, false, false, false, &["list"]),
            domain("hooks", true, false, false, false, &[]),
            domain("mcp_client", true, false, false, false, &[]),
            domain("mcp_server", true, false, false, false, &[]),
            domain("lsp", true, false, false, false, &[]),
            domain("memory", true, false, false, false, &[]),
            domain("cron", true, false, false, false, &[]),
            domain("channels", true, false, false, false, &[]),
            domain("trace", true, false, false, false, &[]),
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
                &["pause", "resume", "exec", "read_file", "write_file"],
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
    }
}
