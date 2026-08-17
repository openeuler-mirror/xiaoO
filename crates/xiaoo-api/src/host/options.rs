//! `SessionOptions` / `LlmOptions`：门面与 60+ 字段 `HostedSessionRuntimeConfig`
//! 之间的降维层。
//!
//! 调用方只声明"差异"，其余由 [`SessionOptions::derive`]（§3.3.3 派生规则）派生。
//! 派生逻辑即现有 endside 组装代码（`cli/entry.rs:956-1038`、
//! `runtime_request.rs:247-326`）与五个解析 helper（§3.3.9）的下沉合并。
//!
//! 设计纪律：本模块**不新增运行时行为**——派生的每个字段值都来自现有 endside
//! 组装路径的提取。差异点见 MR 描述。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::ReasoningEffort;
use lsp::LspServiceRegistry;
use mcp::McpServerConfig;
use xiaoo_shared::backend::GatewayBackendConfig;
use xiaoo_shared::gateway::{
    GatewayEntryContext, HostedSessionRuntimeConfig, SessionOpenRequest, SessionServiceError,
    SubagentRoleConfigEntry,
};

use super::derive::derive_session;

/// LLM 子配置：provider / model / API key / 上下文窗口 / 推理力度。
///
/// `provider` 与 `model` 为必填；其余字段全部可选，缺省由派生填充。
/// 优先级见 `refactor.md` §3.3.3 派生规则表。
#[derive(Clone, Debug)]
pub struct LlmOptions {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) api_base: Option<String>,
    /// 直供 API key（最高优先）。`None` 表示未直供。
    pub(crate) api_key: Option<String>,
    /// 经 secrets 解密的 API key 环境变量名。
    pub(crate) api_key_env: Option<String>,
    /// 显式上下文窗口（跳过探测链）。
    pub(crate) context_window: Option<u32>,
    pub(crate) reasoning_effort: ReasoningEffort,
}

impl LlmOptions {
    /// 构造 LLM 子配置。`provider`（如 `"openai"`/`"anthropic"`）与 `model`
    /// （如 `"qwen3-max"`）为必填。
    pub fn new(provider: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            provider: provider.into(),
            model: model.into(),
            api_base: None,
            api_key: None,
            api_key_env: None,
            context_window: None,
            reasoning_effort: ReasoningEffort::default(),
        }
    }

    /// 自定义 API base URL。覆盖 provider 惯例的默认 base。
    pub fn api_base(mut self, url: impl Into<String>) -> Self {
        self.api_base = Some(url.into());
        self
    }

    /// 直供 API key（最高优先，跳过 secrets 解密）。
    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// 经 secrets 按需解密的 API key 环境变量名（次优先）。
    pub fn api_key_env(mut self, env: impl Into<String>) -> Self {
        self.api_key_env = Some(env.into());
        self
    }

    /// 显式指定上下文窗口（tokens），跳过探测链。
    pub fn context_window(mut self, tokens: u32) -> Self {
        self.context_window = Some(tokens);
        self
    }

    /// 单会话推理力度覆盖（也会作为 `AppTurnRequest.reasoning_effort` 默认值）。
    pub fn reasoning_effort(mut self, effort: ReasoningEffort) -> Self {
        self.reasoning_effort = effort;
        self
    }
}

/// 会话选项：调用方仅声明与缺省不同的部分，其余由 [`SessionOptions::derive`]
/// 派生为 `(SessionOpenRequest, HostedSessionRuntimeConfig)`。
///
/// 派生规则见 `refactor.md` §3.3.3。本阶段**未**为 `system_prompt` / `hooker` /
/// `trace` / `compact_overrides` / `enable_tools` / `max_turns` / `memory_automation`
/// 暴露 builder 方法——这些字段使用类型 `Default`（§3.3.3 派生规则：
/// "feature_flags / token_budget / 类型 Default"）或经派生内部填充。
/// endside 的 TOML 配置 → `SessionOptions` 的翻译层（薄层）在阶段 3 中替代
/// 原两处 60+ 字段的 `HostedSessionRuntimeConfig` 手工组装。
//
// 不 derive `Debug`：`SubagentRoleConfigEntry` 与 `LspServiceRegistry` 均未实现
// `Debug`，且 `SessionOptions` 字段较多，手工实现收益不大。测试中需要观察
// 内部状态时直接访问 `pub(crate)` 字段。
#[derive(Clone)]
pub struct SessionOptions {
    pub(crate) llm: LlmOptions,
    pub(crate) workspace_root: Option<PathBuf>,
    pub(crate) session_id: Option<String>,
    pub(crate) agent_role: Option<String>,
    /// per-tool 开关表。空表示不限制（全开）。
    pub(crate) tools: BTreeMap<String, bool>,
    /// 显式 skills roots，跳过四级优先级解析。
    pub(crate) skills: Option<Vec<PathBuf>>,
    /// 显式 MCP servers，跳过自动发现。传 `Some(vec![])` 显式关闭 MCP。
    pub(crate) mcp_servers: Option<Vec<McpServerConfig>>,
    pub(crate) lsp_registry: Option<Arc<LspServiceRegistry>>,
    pub(crate) backend: Option<GatewayBackendConfig>,
    /// 额外 subagent 角色表，会与内置 plan 角色合并；不允许覆盖 plan。
    /// 偏离 §3.3.3：原文为 `Vec<SubagentRoleConfigEntry>`，但
    /// `SubagentRoleConfigEntry` 无 `role_id` 字段，故取 Map 以匹配
    /// `HostedSessionRuntimeConfig.subagent_roles` 的字段类型与 endside 现有
    /// 组装路径（`cli/entry.rs:1021-1035`、`runtime_request.rs:308-323`）。
    pub(crate) subagent_roles: BTreeMap<String, SubagentRoleConfigEntry>,
    pub(crate) entry: Option<GatewayEntryContext>,
    pub(crate) feature_flags: Option<FeatureFlags>,
    pub(crate) token_budget: Option<TokenBudgetConfig>,
    /// 用户配置的 skills section（extra dirs + allow_scripts）。仅当 `skills`
    /// 未显式设置时，派生内部用此 section 走四级优先级解析。
    pub(crate) skills_section: Option<SkillsSection>,
}

/// 用户 `[skills]` TOML 段模型：额外 skills 目录与是否允许执行脚本。
///
/// 本类型不暴露给 endside 之外的常规调用方；endside 的 TOML 翻译层在
/// `SessionOptions::skills_section` 注入它，使派生与原 `support/config.rs`
/// 的四级优先级解析行为完全一致。
#[derive(Clone, Debug, Default)]
pub struct SkillsSection {
    /// 额外的 skills 目录。
    pub dirs: Vec<PathBuf>,
    /// 是否允许 skill 执行脚本。缺省 `false`。
    pub allow_scripts: bool,
}

impl SessionOptions {
    /// 构造会话选项。LLM 子配置为必填。
    pub fn new(llm: LlmOptions) -> Self {
        Self {
            llm,
            workspace_root: None,
            session_id: None,
            agent_role: None,
            tools: BTreeMap::new(),
            skills: None,
            mcp_servers: None,
            lsp_registry: None,
            backend: None,
            subagent_roles: BTreeMap::new(),
            entry: None,
            feature_flags: None,
            token_budget: None,
            skills_section: None,
        }
    }

    /// 工作区根目录。缺省：进程当前目录。
    pub fn workspace_root(mut self, path: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(path.into());
        self
    }

    /// 指定或恢复既有会话 id。缺省：生成新 id。
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Agent 角色 id（写入 `descriptor.agent_id`）。缺省：`"defaultagent"`。
    pub fn agent_role(mut self, role: impl Into<String>) -> Self {
        self.agent_role = Some(role.into());
        self
    }

    /// per-tool 开关表。空表示不限制（全开）。键必须为内置工具枚举的精确名字
    /// （`load_tool_sources_with_services` 返回的 `tool.spec.name()`）；
    /// 不在枚举里的配置项被静默忽略。
    pub fn tools(mut self, switches: BTreeMap<String, bool>) -> Self {
        self.tools = switches;
        self
    }

    /// 显式 skills roots，跳过四级优先级解析。缺省：四级优先级解析
    /// （§3.3.9 `resolve_skills_config`）。
    pub fn skills(mut self, dirs: Vec<PathBuf>) -> Self {
        self.skills = Some(dirs);
        self
    }

    /// 显式 MCP servers，跳过自动发现。传 `Some(vec![])` 显式关闭 MCP。
    /// 缺省：空 `Vec`（与 endside CLI 的 `mcp_servers: Vec::new()` 一致）。
    pub fn mcp_servers(mut self, servers: Vec<McpServerConfig>) -> Self {
        self.mcp_servers = Some(servers);
        self
    }

    /// LSP 服务注册表。缺省：不启用（`None`）。
    pub fn lsp(mut self, registry: LspServiceRegistry) -> Self {
        self.lsp_registry = Some(Arc::new(registry));
        self
    }

    /// sandbox backend 配置（local/e2b/conch）。缺省：`GatewayBackendConfig::new("local", …)`。
    pub fn backend(mut self, config: GatewayBackendConfig) -> Self {
        self.backend = Some(config);
        self
    }

    /// 额外 subagent 角色表，与内置 plan 角色合并。**不允许**覆盖 plan 角色
    /// （键 `"plan"` 触发 `SessionOptionsError`）。
    pub fn subagent_roles(mut self, roles: BTreeMap<String, SubagentRoleConfigEntry>) -> Self {
        self.subagent_roles = roles;
        self
    }

    /// 入口来源标记。缺省：`GatewayEntryContext::cli()`。
    pub fn entry(mut self, entry: GatewayEntryContext) -> Self {
        self.entry = Some(entry);
        self
    }

    /// feature flags。缺省：`FeatureFlags::default()`（`tool_execution = true` 等）。
    pub fn feature_flags(mut self, flags: FeatureFlags) -> Self {
        self.feature_flags = Some(flags);
        self
    }

    /// token budget。缺省：由 `resolve_context_window` 派生 `total_budget`，
    /// `reserved_for_output = total_budget / 10`、`reserved_for_system = total_budget / 20`、
    /// `hard_limit_ratio = 0.9`（与 endside CLI 行为一致）。
    pub fn token_budget(mut self, budget: TokenBudgetConfig) -> Self {
        self.token_budget = Some(budget);
        self
    }

    /// 用户 `[skills]` TOML 段（extra dirs + allow_scripts）。仅当 `skills`
    /// 未显式设置时生效；派生内部用此 section 走四级优先级解析。
    /// 缺省：`None`（仅用四级默认目录，无用户额外目录）。
    ///
    /// 偏离 §3.3.3：原文未列出本方法。补充它是为让 §3.3.9 的
    /// `resolve_skills_config(workspace_root, section)` 的 `section` 参数在
    /// 生产路径（endside 翻译层注入 TOML `[skills]` 段）中有用武之地，否则
    /// 该参数将退化为永远 `None` 的死代码。
    pub fn skills_section(mut self, section: SkillsSection) -> Self {
        self.skills_section = Some(section);
        self
    }

    /// 派生为 `(SessionOpenRequest, HostedSessionRuntimeConfig)`。
    ///
    /// 派生规则见 `refactor.md` §3.3.3；实现对照基准为
    /// `apps/endside/src/cli/entry.rs:956-1038` 与
    /// `apps/endside/src/gateway_api/runtime_request.rs:247-326`。
    ///
    /// 错误折叠进 `SessionServiceError`（§3.3.6）：LLM provider / 压缩管道
    /// 构建失败 → `RuntimeBuild`；覆盖内置 plan 角色 → `InvalidRequest`。
    pub async fn derive(self) -> Result<(SessionOpenRequest, HostedSessionRuntimeConfig), SessionServiceError> {
        derive_session(self).await
    }
}

/// `SessionOptions::derive` 的失败原因（折叠进 `SessionServiceError`，§3.3.6）。
///
/// 本类型当前为占位——§3.3.6 明确"不为此新造第三个错误类型"。保留为枚举
/// 仅为后续阶段评审是否需要独立错误类型时使用，当前所有派生失败都经
/// `SessionServiceError` 表达。
#[derive(Debug, thiserror::Error)]
pub enum SessionOptionsError {
    /// 其它未分类的派生失败。当前所有失败均折叠进 `SessionServiceError`，
    /// 本变体保留供后续阶段评审使用。
    #[error("session options derive failed: {0}")]
    Other(String),
}
