//! MCP 配置词汇与配置合并工具再导出。
//!
//! 应用层（endside support/config、serverside daemon_config）需要命名
//! [`McpSection`] / [`McpServerConfig`] 以 serde 反序列化与持有配置字段，
//! 并复用 [`resolve_json_config_path`] / [`load_json_servers`] /
//! [`merge_server_configs`] 把 toml + json 两路 MCP 配置合并为运行时
//! 服务器列表——无法下沉为粗粒度函数（应用必须命名这些类型与函数签名），
//! 故作为配置词汇与配套函数再导出。`McpConfigError` 是这些函数的错误类型，
//! 随函数签名可达。其余 mcp 内部实现类型不向应用导出。
//!
//! 待办：endside 与 serverside 各自复刻了 `load_merged_mcp_servers` 近似
//! 副本，可进一步合并为 shared 单一粗粒度入口；届时这些函数的再导出可
//! 移除。

pub use mcp::{
    load_json_servers, merge_server_configs, resolve_json_config_path, McpConfigError, McpSection,
    McpServerConfig, Transport,
};

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct McpServerStatus {
    pub name: String,
    pub transport: &'static str,
    pub enabled: bool,
    pub target: Option<String>,
    pub timeout_ms: u64,
    pub bearer_token_env: Option<String>,
    pub agent_id: Option<String>,
    pub header_names: Vec<String>,
    pub effect: McpEffectSummary,
    pub state: &'static str,
    pub protocol_version: Option<String>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub tools: Vec<McpToolSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct McpEffectSummary {
    pub reads_filesystem: bool,
    pub writes_filesystem: bool,
    pub network_access: bool,
    pub side_effects: bool,
}

#[derive(Debug, Serialize)]
pub struct McpToolSummary {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

pub async fn inspect_mcp_servers(servers: &[McpServerConfig]) -> Vec<McpServerStatus> {
    let mut reports = Vec::with_capacity(servers.len());
    for server in servers {
        reports.push(inspect_mcp_server(server).await);
    }
    reports
}

async fn inspect_mcp_server(server: &McpServerConfig) -> McpServerStatus {
    let mut report = base_server_status(server);
    if !server.is_enabled() {
        report.state = "disabled";
        return report;
    }
    let client = match mcp::McpClient::connect(server).await {
        Ok(client) => client,
        Err(error) => {
            report.state = "error";
            report.error = Some(sanitized_error(server, error));
            return report;
        }
    };
    let initialized = match client.initialize().await {
        Ok(initialized) => initialized,
        Err(error) => {
            report.state = "error";
            report.error = Some(sanitized_error(server, error));
            let _ = client.close().await;
            return report;
        }
    };
    report.protocol_version = Some(initialized.protocol_version);
    if let Some(info) = initialized.server_info {
        report.server_name = Some(info.name);
        report.server_version = Some(info.version);
    }
    match client.list_tools().await {
        Ok(tools) => {
            report.state = "connected";
            report.tools = tools
                .into_iter()
                .map(|tool| McpToolSummary {
                    name: tool.name,
                    description: tool.description,
                    input_schema: tool.input_schema,
                })
                .collect();
        }
        Err(error) => {
            report.state = "error";
            report.error = Some(sanitized_error(server, error));
        }
    }
    if let Err(error) = client.close().await {
        if report.error.is_none() {
            report.error = Some(format!("failed to close MCP connection: {error}"));
        }
    }
    report
}

fn base_server_status(server: &McpServerConfig) -> McpServerStatus {
    McpServerStatus {
        name: server.name.clone(),
        transport: match server.transport {
            Transport::Stdio => "stdio",
            Transport::Sse => "sse",
            Transport::StreamableHttp => "streamable_http",
        },
        enabled: server.is_enabled(),
        target: match server.transport {
            Transport::Stdio => server.command.clone(),
            Transport::Sse | Transport::StreamableHttp => {
                server.url.as_deref().and_then(sanitized_http_target)
            }
        },
        timeout_ms: server.timeout_ms,
        bearer_token_env: server.bearer_token_env.clone(),
        agent_id: server.agent_id.clone(),
        header_names: server.headers.keys().cloned().collect(),
        effect: McpEffectSummary {
            reads_filesystem: server.effect.reads_filesystem,
            writes_filesystem: server.effect.writes_filesystem,
            network_access: server.effect.network_access,
            side_effects: server.effect.side_effects,
        },
        state: "connecting",
        protocol_version: None,
        server_name: None,
        server_version: None,
        tools: Vec::new(),
        error: None,
    }
}

fn sanitized_http_target(value: &str) -> Option<String> {
    let mut url = reqwest::Url::parse(value).ok()?;
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Some(url.to_string())
}

fn sanitized_error(server: &McpServerConfig, error: impl std::fmt::Display) -> String {
    let mut message = error.to_string();
    if let Some(url) = server.url.as_deref() {
        if let Some(safe_url) = sanitized_http_target(url) {
            message = message.replace(url, &safe_url);
        }
    }
    message
}

#[cfg(test)]
#[path = "../../../tests/unit/shared/mcp_support_test.rs"]
mod management_tests;
