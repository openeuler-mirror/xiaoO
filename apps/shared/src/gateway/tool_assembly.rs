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

/// 不透明的已绑定控制句柄存储：跨多次工具装配复用同一份控制绑定
/// （由 `AppBootstrap` 在启动期注入一次）。底层控制 trait 只出现在本结构体
/// 内部方法签名里，不向应用导出——应用持有 [`BoundControlStore`] 并经它
/// 传值，无需 `use` 任何底层 trait 名。
#[derive(Clone, Default)]
pub struct BoundControlStore {
    inner: Arc<std::sync::RwLock<Option<Arc<dyn SubagentControl>>>>,
}

impl BoundControlStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注入一份控制句柄（启动期由 `AppBootstrap` 调一次）。
    pub fn set(&self, control: Arc<dyn SubagentControl>) {
        *self
            .inner
            .write()
            .expect("bound control store lock should not be poisoned") = Some(control);
    }

    /// 取当前绑定的控制句柄（未绑定返回 `None`）。
    pub fn snapshot(&self) -> Option<Arc<dyn SubagentControl>> {
        self.inner
            .read()
            .expect("bound control store lock should not be poisoned")
            .clone()
    }
}

/// 不透明的 MCP 工具缓存：持有已初始化（connect + initialize + list_tools）
/// 的 MCP 服务器连接，使多次工具装配共用同一次初始化结果。
///
/// `McpServerWithTools` 是底层句柄类型，只出现在本结构体内部字段里，不向应用
/// 导出——应用持有 [`McpToolCache`] 即可，无需 `use` 任何底层 crate。
#[derive(Clone, Default)]
pub struct McpToolCache {
    inner: Arc<tokio::sync::Mutex<Option<Vec<mcp::McpServerWithTools>>>>,
}

impl McpToolCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// 取已初始化的工具（未初始化返回 `None`）。仅做读，不触发网络。
    pub async fn snapshot(&self) -> Option<Vec<mcp::McpServerWithTools>> {
        self.inner.lock().await.clone()
    }

    /// 若尚未初始化则用 `configs` 初始化并缓存；返回克隆的工具列表。
    /// 与 [`hosted_runtime_resolver`](super::hosted_runtime_resolver) 的
    /// 双重检查 lazy init 语义一致：配置为空 → 直接缓存空列表（避免误判未初始化）。
    async fn get_or_init(
        &self,
        configs: &[McpServerConfig],
    ) -> Result<Vec<mcp::McpServerWithTools>, ToolAssemblyError> {
        if let Some(existing) = self.inner.lock().await.clone() {
            return Ok(existing);
        }
        let mut guard = self.inner.lock().await;
        if let Some(existing) = guard.clone() {
            return Ok(existing);
        }
        let initialised = if configs.is_empty() {
            Vec::new()
        } else {
            mcp::init_mcp_tools(configs).await
        };
        *guard = Some(initialised.clone());
        Ok(initialised)
    }
}

/// 工具装配输入。字段只用基本类型和 shared 已导出的句柄。
///
/// `lsp_registry` 是配置类型（serverside 配置解析后持有）；
/// `subagent_control` 是不透明句柄（serverside 从 shared `CoreBackedSessionService`
/// 拿到后传值，无需 `use` trait 名）；
/// `mcp_servers` 是 MCP 配置，`mcp_cache` 是跨装配复用的初始化缓存
/// （两者同时给时优先用缓存；缓存未命中则按 `mcp_servers` 初始化并写回缓存）。
#[derive(Clone, Default)]
pub struct ToolAssemblyInput {
    pub workspace_root: Option<PathBuf>,
    pub disable_plugin_tools: bool,
    pub lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub mcp_servers: Option<Vec<McpServerConfig>>,
    pub mcp_cache: Option<McpToolCache>,
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

/// 解析本装配所需的 MCP 工具：优先用 `input.mcp_cache`（跨装配复用初始化），
/// 否则按 `input.mcp_servers` 直接初始化（一次性，不缓存）。
///
/// `None` 表示"未提供 MCP 配置"语义（对应底层 `ToolRuntimeServices.mcp_servers`
/// 为 `None`，即不接入 MCP 工具源）；`Some(vec)` 表示已初始化（即便为空，
/// 表示配置为空或所有服务器不可达，避免调用方误判未初始化）。
async fn resolve_mcp_tools(
    input: &ToolAssemblyInput,
) -> Result<Option<Vec<mcp::McpServerWithTools>>, ToolAssemblyError> {
    match (&input.mcp_cache, &input.mcp_servers) {
        (Some(cache), Some(configs)) => Ok(Some(cache.get_or_init(configs).await?)),
        (None, Some(configs)) if configs.is_empty() => Ok(Some(Vec::new())),
        (None, Some(configs)) => {
            let initialised = mcp::init_mcp_tools(configs).await;
            Ok(Some(initialised))
        }
        _ => Ok(None),
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
    let mcp_servers = resolve_mcp_tools(input).await?;
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
    let mcp_servers = resolve_mcp_tools(&input).await?;
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
