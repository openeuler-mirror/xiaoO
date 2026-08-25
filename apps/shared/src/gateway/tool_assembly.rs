//! 工具装配门面。
//!
//! serverside 的 `build_tool_registry` / `build_tool_runtime_services` 与 shared
//! resolver 内已有逻辑（`hosted_runtime_resolver`）合并为单一实现，对外只
//! pub 两个入口（[`discover_tool_names`] / [`build_tool_registry`]）与两个
//! 粗粒度输入结构（[`ToolAssemblyInput`] / [`SubagentRoleSpec`]）。
//!
//! 两条硬约束：
//! - 入参出参不出现底层细粒度类型：`ToolSource` / `ToolRuntimeServices` /
//!   `ToolRegistryBuilderImpl` / `ToolRegistryConfig` / `ToolVisibilityConfig` /
//!   `SubagentRoleConfig` 一律在本模块内部使用（`pub(crate)` 级别），不暴露。
//! - 新增 pub 项必须有应用消费者：endside `runtime_request::resolve_visible_tool_names`
//!   消费 `discover_tool_names`；serverside `daemon_runtime` 装配消费
//!   `build_tool_registry`。
//!
//! `SubagentControl` 类型名只出现在本模块的签名里（`ToolAssemblyInput.subagent_control`
//! 字段），不向应用再导出：serverside 拿到 shared 的 `CoreBackedSessionService`
//! 后作为值传入字段，unsized coercion 不需要 `use` trait 名。

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use agent_contracts::ToolRegistryBuilder;
use agent_types::common::ids::{AgentId, ToolName};
use agent_types::tool::{ToolRegistryConfig, ToolVisibilityConfig};
use lsp::LspServiceRegistry;
use mcp::McpServerConfig;
use subagent::SubagentControl;
use tool::{
    load_tool_sources_with_services, SubagentRoleConfig, ToolRegistryBuilderImpl,
    ToolRuntimeServices,
};
use xiaoo_api::tools::ToolRegistry;

/// shared 自有的 subagent 角色描述（代替暴露 `tool::SubagentRoleConfig`）。
///
/// 字段全是基本类型 / 基本集合，应用从自己的配置结构就能直接填，不需要
/// `use` 任何底层 crate。`tools` 用 `BTreeMap<String, bool>` 表达与
/// `tool::SubagentRoleConfig` 完全一致的"按名开关"语义。
#[derive(Clone, Default)]
pub struct SubagentRoleSpec {
    pub description: String,
    pub prompt: Option<String>,
    pub max_turns: Option<u32>,
    pub tools: BTreeMap<String, bool>,
}

/// 工具装配输入。字段只用基本类型和 shared 已导出的句柄。
///
/// `lsp_registry` / `mcp_servers` 是配置类型（serverside 配置解析后持有）；
/// `subagent_control` 是不透明句柄（serverside 从 shared `CoreBackedSessionService`
/// 拿到后传值，无需 `use` trait 名）。
#[derive(Clone, Default)]
pub struct ToolAssemblyInput {
    pub workspace_root: Option<PathBuf>,
    pub disable_plugin_tools: bool,
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    pub subagent_roles: BTreeMap<String, SubagentRoleSpec>,
    pub subagent_control: Option<Arc<dyn SubagentControl>>,
}

/// 工具装配错误。
#[derive(Debug, thiserror::Error)]
pub enum ToolAssemblyError {
    #[error("failed to initialise MCP servers: {0}")]
    McpInit(String),
    #[error("failed to build tool registry: {0}")]
    Build(String),
}

/// 把装配输入翻译成底层 `ToolRuntimeServices`（内部细节，不导出）。
fn to_runtime_services(
    input: &ToolAssemblyInput,
    mcp_servers: Option<Vec<mcp::McpServerWithTools>>,
) -> ToolRuntimeServices {
    let subagent_roles = input
        .subagent_roles
        .iter()
        .map(|(role_id, spec)| {
            (
                role_id.clone(),
                SubagentRoleConfig {
                    description: spec.description.clone(),
                    prompt: spec.prompt.clone(),
                    max_turns: spec.max_turns,
                    tools: spec.tools.clone(),
                },
            )
        })
        .collect();
    ToolRuntimeServices {
        disable_plugin_tools: input.disable_plugin_tools,
        subagent_control: input.subagent_control.clone(),
        lsp_registry: input.lsp_registry.clone(),
        workspace_root: input.workspace_root.clone(),
        subagent_roles,
        mcp_servers,
        ..ToolRuntimeServices::default()
    }
}

/// 懒初始化 MCP 服务器（连接 + initialize + list_tools），返回底层
/// `McpServerWithTools` 列表。`None` 表示"未初始化"语义。
///
/// 与 `hosted_runtime_resolver` 的 lazy init 语义一致：传入的配置为空 → 直接
/// 返回 `Some(vec![])`（init 已完成，即便结果为空），避免调用方误判未初始化。
async fn init_mcp_servers(
    mcp_servers: &Option<Vec<McpServerConfig>>,
) -> Result<Option<Vec<mcp::McpServerWithTools>>, ToolAssemblyError> {
    match mcp_servers {
        None => Ok(None),
        Some(configs) if configs.is_empty() => Ok(Some(Vec::new())),
        Some(configs) => {
            let initialised = mcp::init_mcp_tools(configs).await;
            Ok(Some(initialised))
        }
    }
}

/// 枚举当前装配输入下可发现的全部工具名。
///
/// endside `runtime_request::resolve_visible_tool_names` 只需要这个：拿到
/// 全量工具名集合后，与角色配置的开关表做交集，得到可见工具名。
///
/// `async` 是因为 MCP 服务器源需要连接 + 初始化后才能 list_tools
/// （endside 当前调用点不传 mcp_servers，故实际不触发网络）。
pub async fn discover_tool_names(
    input: &ToolAssemblyInput,
) -> Result<Vec<ToolName>, ToolAssemblyError> {
    let mcp_servers = init_mcp_servers(&input.mcp_servers).await?;
    let services = to_runtime_services(input, mcp_servers);
    let tool_sources = load_tool_sources_with_services(services);
    let names = tool_sources
        .iter()
        .flat_map(|source| source.discover())
        .map(|tool| tool.spec.name().clone())
        .collect();
    Ok(names)
}

/// 装配工具注册表；可见性过滤用粗粒度参数表达（每 agent 允许的工具名），
/// 内部翻译成底层 `ToolVisibilityConfig`。返回 `Arc<dyn ToolRegistry>` 用
/// `xiaoo_api::tools` 既有导出（不违反 C1）。
pub async fn build_tool_registry(
    input: ToolAssemblyInput,
    per_agent_allowed_tools: HashMap<AgentId, Vec<ToolName>>,
) -> Result<Arc<dyn ToolRegistry>, ToolAssemblyError> {
    let mcp_servers = init_mcp_servers(&input.mcp_servers).await?;
    let services = to_runtime_services(&input, mcp_servers);
    let tool_sources = load_tool_sources_with_services(services);

    let registry = ToolRegistryBuilderImpl::new()
        .with_sources(tool_sources)
        .with_config(ToolRegistryConfig {
            visibility: ToolVisibilityConfig {
                per_agent_allowed_tools,
            },
        })
        .build()
        .map_err(|e| ToolAssemblyError::Build(e.to_string()))?;

    Ok(Arc::from(registry))
}
