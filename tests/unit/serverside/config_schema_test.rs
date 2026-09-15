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
