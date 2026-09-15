use serde::Serialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crate::daemon_config::DaemonConfig;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpServerIssue {
    pub path: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpServerEndpointStatus {
    pub endpoint: String,
    pub bearer_token_env: String,
    pub token_available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpChatbotStatus {
    #[serde(flatten)]
    pub endpoint: McpServerEndpointStatus,
    pub workspace: String,
    pub workspace_state: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpAgentStatus {
    #[serde(flatten)]
    pub endpoint: McpServerEndpointStatus,
    pub agent_role: Option<String>,
    pub role_available: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct McpServerReport {
    pub schema_version: u32,
    pub enabled: bool,
    pub valid: bool,
    pub idle_timeout_secs: u64,
    pub reaper_interval_secs: u64,
    pub allowed_origins: Vec<String>,
    pub backend_kind: String,
    pub tokens_distinct: Option<bool>,
    pub chatbot: McpChatbotStatus,
    pub agent: McpAgentStatus,
    pub errors: Vec<McpServerIssue>,
}

pub fn mcp_server_report(config: &DaemonConfig) -> McpServerReport {
    let source = &config.app.mcp_server;
    let chatbot_env = normalized(source.chatbot.bearer_token_env.as_deref())
        .unwrap_or("XIAOO_MCP_CHATBOT_TOKEN")
        .to_string();
    let agent_env = normalized(source.agent.bearer_token_env.as_deref())
        .unwrap_or("XIAOO_MCP_AGENT_TOKEN")
        .to_string();
    let chatbot_token = environment_secret(&chatbot_env);
    let agent_token = environment_secret(&agent_env);
    let agent_role = normalized(source.agent.agent_role.as_deref()).map(str::to_string);
    let role_available = agent_role
        .as_ref()
        .is_none_or(|role| config.app.agent.contains_key(role));
    let workspace =
        normalized(source.chatbot.workspace.as_deref()).unwrap_or("~/.xiaoo/mcp-chatbot-empty");
    let workspace_path = PathBuf::from(shellexpand::tilde(workspace).into_owned());
    let workspace_state = workspace_state(&workspace_path);
    let idle_timeout_secs = source.idle_timeout_secs.unwrap_or(600);
    let reaper_interval_secs = source.reaper_interval_secs.unwrap_or(30);
    let backend_kind = config
        .server_operation_backend()
        .map(|backend| backend.kind)
        .unwrap_or_else(|| "local".to_string());
    let tokens_distinct = chatbot_token
        .as_ref()
        .zip(agent_token.as_ref())
        .map(|(chatbot, agent)| chatbot != agent);
    let allowed_origins = source
        .allowed_origins
        .iter()
        .filter_map(|origin| normalized(Some(origin.as_str())))
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let mut errors = Vec::new();

    if source.enabled {
        if backend_kind != "local" {
            issue(
                &mut errors,
                "server.operation_backend.kind",
                "unsupported_backend",
                "MCP Agent 只支持 local 运行后端",
            );
        }
        if idle_timeout_secs == 0 {
            issue(
                &mut errors,
                "mcp_server.idle_timeout_secs",
                "out_of_range",
                "idle_timeout_secs 必须大于 0",
            );
        }
        if reaper_interval_secs == 0 {
            issue(
                &mut errors,
                "mcp_server.reaper_interval_secs",
                "out_of_range",
                "reaper_interval_secs 必须大于 0",
            );
        }
        if chatbot_token.is_none() {
            issue(
                &mut errors,
                "mcp_server.chatbot.bearer_token_env",
                "missing_secret",
                format!("缺少环境变量或插件密钥 {chatbot_env}"),
            );
        }
        if agent_token.is_none() {
            issue(
                &mut errors,
                "mcp_server.agent.bearer_token_env",
                "missing_secret",
                format!("缺少环境变量或插件密钥 {agent_env}"),
            );
        }
        if tokens_distinct == Some(false) {
            issue(
                &mut errors,
                "mcp_server",
                "duplicate_tokens",
                "Chatbot 与 Agent Bearer Token 必须不同",
            );
        }
        if !role_available {
            issue(
                &mut errors,
                "mcp_server.agent.agent_role",
                "unknown_role",
                format!(
                    "Agent 角色 {} 不存在",
                    agent_role.as_deref().unwrap_or_default()
                ),
            );
        }
        if matches!(
            workspace_state.as_str(),
            "not_directory" | "not_empty" | "unreadable"
        ) {
            issue(
                &mut errors,
                "mcp_server.chatbot.workspace",
                "invalid_workspace",
                format!("Chatbot 工作目录状态无效：{workspace_state}"),
            );
        }
    }

    McpServerReport {
        schema_version: 1,
        enabled: source.enabled,
        valid: errors.is_empty(),
        idle_timeout_secs,
        reaper_interval_secs,
        allowed_origins,
        backend_kind,
        tokens_distinct,
        chatbot: McpChatbotStatus {
            endpoint: McpServerEndpointStatus {
                endpoint: "/mcp/chatbot".to_string(),
                bearer_token_env: chatbot_env,
                token_available: chatbot_token.is_some(),
            },
            workspace: workspace_path.display().to_string(),
            workspace_state,
        },
        agent: McpAgentStatus {
            endpoint: McpServerEndpointStatus {
                endpoint: "/mcp/agent".to_string(),
                bearer_token_env: agent_env,
                token_available: agent_token.is_some(),
            },
            agent_role,
            role_available,
        },
        errors,
    }
}

fn normalized(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn environment_secret(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

fn workspace_state(path: &Path) -> String {
    if !path.exists() {
        return "missing".to_string();
    }
    if !path.is_dir() {
        return "not_directory".to_string();
    }
    match fs::read_dir(path) {
        Ok(mut entries) => match entries.next() {
            None => "empty".to_string(),
            Some(Ok(_)) => "not_empty".to_string(),
            Some(Err(_)) => "unreadable".to_string(),
        },
        Err(_) => "unreadable".to_string(),
    }
}

fn issue(errors: &mut Vec<McpServerIssue>, path: &str, code: &str, message: impl Into<String>) {
    errors.push(McpServerIssue {
        path: path.to_string(),
        code: code.to_string(),
        message: message.into(),
    });
}

#[cfg(test)]
#[path = "../../../tests/unit/serverside/mcp_server_management_test.rs"]
mod tests;
