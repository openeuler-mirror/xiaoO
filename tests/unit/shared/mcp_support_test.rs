use super::{inspect_mcp_servers, McpSection};

#[tokio::test]
async fn disabled_server_is_reported_without_connecting() {
    let section: McpSection = toml::from_str(
        r#"
[[servers]]
name = "disabled-docs"
transport = "stdio"
command = "command-that-must-not-run"
enabled = false
"#,
    )
    .expect("MCP config");

    let reports = inspect_mcp_servers(&section.servers).await;
    assert_eq!(reports[0].state, "disabled");
    assert!(reports[0].tools.is_empty());
    assert!(reports[0].error.is_none());
}
