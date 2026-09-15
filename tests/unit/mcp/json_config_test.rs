use super::{load_json_servers, merge_server_configs, parse_mcp_json, resolve_json_config_path};
use crate::{McpSection, Transport};
use std::path::Path;
use std::sync::Mutex;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn server(name: &str) -> crate::McpServerConfig {
    toml::from_str::<McpSection>(&format!(
        r#"
[[servers]]
name = {name:?}
transport = "stdio"
command = "test-server"
"#
    ))
    .unwrap()
    .servers
    .remove(0)
}

fn write_json(path: &Path, name: &str) {
    std::fs::write(
        path,
        format!(r#"{{"mcpServers":{{"{name}":{{"transport":"stdio","command":"test-server"}}}}}}"#),
    )
    .unwrap();
}

#[test]
fn parses_streamable_http_mcp_json_without_resolving_secret_value() {
    let section = parse_mcp_json(
        r#"{
                "mcpServers": {
                    "ram-a": {
                        "transport": "streamable_http",
                        "url": "http://127.0.0.1:18081/mcp",
                        "bearer_token_env": "RAM_A_TOKEN",
                        "agent_id": "xiaoo",
                        "headers": {"X-XiaoO-Client": "ram-a"}
                    }
                }
            }"#,
    )
    .unwrap();

    assert_eq!(section.servers.len(), 1);
    assert_eq!(section.servers[0].name, "ram-a");
    assert_eq!(section.servers[0].transport, Transport::StreamableHttp);
    assert_eq!(
        section.servers[0].bearer_token_env.as_deref(),
        Some("RAM_A_TOKEN")
    );
    assert_eq!(section.servers[0].agent_id.as_deref(), Some("xiaoo"));
    assert_eq!(
        section.servers[0]
            .headers
            .get("X-XiaoO-Client")
            .map(String::as_str),
        Some("ram-a")
    );
}

#[test]
fn json_server_names_have_deterministic_key_order() {
    let section = parse_mcp_json(
        r#"{"mcpServers":{
                "z-last":{"transport":"stdio","command":"z"},
                "a-first":{"transport":"stdio","command":"a"}
            }}"#,
    )
    .unwrap();

    let names = section
        .servers
        .iter()
        .map(|server| server.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["a-first", "z-last"]);
}

#[test]
fn rejects_unknown_json_server_fields() {
    let error = parse_mcp_json(
        r#"{"mcpServers":{"ram-a":{
                "transport":"streamable_http",
                "url":"http://127.0.0.1:18081/mcp",
                "bearer_token":"literal-secret"
            }}}"#,
    )
    .unwrap_err();

    assert!(error.to_string().contains("bearer_token"));
}

#[test]
fn rejects_authorization_header_value_in_json() {
    let error = parse_mcp_json(
        r#"{"mcpServers":{"ram-a":{
                "transport":"streamable_http",
                "url":"http://127.0.0.1:18081/mcp",
                "headers":{"Authorization":"Bearer literal-secret"}
            }}}"#,
    )
    .unwrap_err();

    assert!(error.to_string().contains("Authorization"));
    assert!(!error.to_string().contains("literal-secret"));
}

#[test]
fn rejects_transport_managed_headers_in_json() {
    for name in [
        "Origin",
        "Mcp-Session-Id",
        "MCP-Protocol-Version",
        "Accept",
        "Content-Type",
        "X-Agent-ID",
        "Last-Event-ID",
    ] {
        let content = serde_json::json!({
            "mcpServers": {
                "ram-a": {
                    "transport": "streamable_http",
                    "url": "http://127.0.0.1:18081/mcp",
                    "headers": {name: "override"}
                }
            }
        })
        .to_string();

        let error = parse_mcp_json(&content).unwrap_err();
        assert!(
            error.to_string().contains(name),
            "header {name} unexpectedly accepted"
        );
        assert!(!error.to_string().contains("override"));
    }
}

#[test]
fn rejects_invalid_url_and_zero_timeout() {
    let url_error = parse_mcp_json(
        r#"{"mcpServers":{"bad":{
                "transport":"streamable_http",
                "url":"file:///tmp/socket"
            }}}"#,
    )
    .unwrap_err();
    assert!(url_error.to_string().contains("url"));

    let timeout_error = parse_mcp_json(
        r#"{"mcpServers":{"bad":{
                "transport":"stdio",
                "command":"test-server",
                "timeout_ms":0
            }}}"#,
    )
    .unwrap_err();
    assert!(timeout_error.to_string().contains("timeout_ms"));
}

#[test]
fn duplicate_toml_and_json_server_names_fail_with_sources() {
    let error = merge_server_configs(
        vec![server("ram-a")],
        vec![server("ram-a")],
        "config.toml",
        ".mcp.json",
    )
    .unwrap_err();

    assert!(error.to_string().contains("ram-a"));
    assert!(error.to_string().contains("config.toml"));
    assert!(error.to_string().contains(".mcp.json"));
}

#[test]
fn explicit_path_precedes_environment_and_workspace() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(home.join(".config/xiaoo")).unwrap();

    let explicit = root.path().join("explicit.json");
    let environment = root.path().join("environment.json");
    write_json(&explicit, "explicit");
    write_json(&environment, "environment");
    write_json(&workspace.join(".mcp.json"), "workspace");
    write_json(&home.join(".config/xiaoo/mcp.json"), "home");

    let old = std::env::var_os("XIAOO_MCP_CONFIG");
    std::env::set_var("XIAOO_MCP_CONFIG", &environment);
    let resolved = resolve_json_config_path(Some(&explicit), &workspace, Some(&home));
    let servers = load_json_servers(Some(&explicit), &workspace, Some(&home)).unwrap();
    if let Some(old) = old {
        std::env::set_var("XIAOO_MCP_CONFIG", old);
    } else {
        std::env::remove_var("XIAOO_MCP_CONFIG");
    }

    assert_eq!(resolved.as_deref(), Some(explicit.as_path()));
    assert_eq!(servers[0].name, "explicit");
}

#[test]
fn environment_precedes_workspace_and_workspace_precedes_home() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    let home = root.path().join("home");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir_all(home.join(".config/xiaoo")).unwrap();

    let environment = root.path().join("environment.json");
    write_json(&environment, "environment");
    write_json(&workspace.join(".mcp.json"), "workspace");
    write_json(&home.join(".config/xiaoo/mcp.json"), "home");

    let old = std::env::var_os("XIAOO_MCP_CONFIG");
    std::env::set_var("XIAOO_MCP_CONFIG", &environment);
    let environment_servers = load_json_servers(None, &workspace, Some(&home)).unwrap();
    std::env::remove_var("XIAOO_MCP_CONFIG");
    let workspace_servers = load_json_servers(None, &workspace, Some(&home)).unwrap();
    std::fs::remove_file(workspace.join(".mcp.json")).unwrap();
    let home_servers = load_json_servers(None, &workspace, Some(&home)).unwrap();
    if let Some(old) = old {
        std::env::set_var("XIAOO_MCP_CONFIG", old);
    }

    assert_eq!(environment_servers[0].name, "environment");
    assert_eq!(workspace_servers[0].name, "workspace");
    assert_eq!(home_servers[0].name, "home");
}

#[test]
fn malformed_discovered_json_is_fatal() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join(".mcp.json"), "{not-json").unwrap();

    let old = std::env::var_os("XIAOO_MCP_CONFIG");
    std::env::remove_var("XIAOO_MCP_CONFIG");
    let error = load_json_servers(None, &workspace, None).unwrap_err();
    if let Some(old) = old {
        std::env::set_var("XIAOO_MCP_CONFIG", old);
    }

    assert!(error.to_string().contains(".mcp.json"));
}

#[test]
fn invalid_discovered_json_server_reports_source_path() {
    let _guard = ENV_LOCK.lock().unwrap();
    let root = TempDir::new().unwrap();
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let path = workspace.join(".mcp.json");
    std::fs::write(
        &path,
        r#"{"mcpServers":{"bad":{"transport":"streamable_http","url":"file:///tmp/mcp"}}}"#,
    )
    .unwrap();

    let old = std::env::var_os("XIAOO_MCP_CONFIG");
    std::env::remove_var("XIAOO_MCP_CONFIG");
    let error = load_json_servers(None, &workspace, None).unwrap_err();
    if let Some(old) = old {
        std::env::set_var("XIAOO_MCP_CONFIG", old);
    }

    assert!(error.to_string().contains(path.to_string_lossy().as_ref()));
}
