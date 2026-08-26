use crate::config_schema::CONFIG_SCHEMA_VERSION;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagementCapabilities {
    pub api_version: u32,
    pub config_schema_version: u32,
    pub config_commands: Vec<&'static str>,
    pub domains: Vec<ManagementDomainCapability>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ManagementDomainCapability {
    pub id: &'static str,
    pub configurable: bool,
    pub runtime_read: bool,
    pub runtime_write: bool,
    pub test: bool,
    pub actions: Vec<&'static str>,
}

pub fn management_capabilities() -> ManagementCapabilities {
    ManagementCapabilities {
        api_version: 1,
        config_schema_version: CONFIG_SCHEMA_VERSION,
        config_commands: vec![
            "config schema",
            "config providers",
            "config validate",
            "config inspect",
        ],
        domains: vec![
            domain(
                "config",
                true,
                false,
                false,
                true,
                &["schema", "providers", "validate", "inspect"],
            ),
            domain("models", true, false, true, false, &["session_select"]),
            domain("agents", true, false, false, false, &[]),
            domain("roles", true, false, false, false, &[]),
            domain("subagents", true, false, false, false, &[]),
            domain("tools", true, false, false, false, &[]),
            domain("skills", true, false, false, false, &[]),
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
        id,
        configurable,
        runtime_read,
        runtime_write,
        test,
        actions: actions.to_vec(),
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
        assert!(models.actions.contains(&"session_select"));

        let skills = capabilities
            .domains
            .iter()
            .find(|domain| domain.id == "skills")
            .expect("skills capability");
        assert!(skills.configurable);
        assert!(!skills.runtime_read);
        assert!(!skills.runtime_write);
        assert!(!skills.test);
    }
}
