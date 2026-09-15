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

    let vault = capabilities
        .domains
        .iter()
        .find(|domain| domain.id == "vault")
        .expect("Vault capability");
    assert!(vault.runtime_read);
    assert!(vault.runtime_write);
    assert!(vault.test);
    assert!(vault.actions.iter().any(|action| action == "inspect"));
    assert!(vault
        .actions
        .iter()
        .any(|action| action == "list_references"));
    assert!(vault.actions.iter().any(|action| action == "rotate"));
    assert!(vault.actions.iter().any(|action| action == "delete"));

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
