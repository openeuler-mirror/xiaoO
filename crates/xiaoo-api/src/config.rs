//! 共用配置件：MCP / skills / LSP / 内置角色 / cron 表达式校验。
//!
//! 【L1 支撑类型层】
//!
//! > 有意**不**新造统一的 `LlmConfig` / `AgentRoleConfig` 等 TOML 配置结构
//! > （`refactor.md` §3.4.10）：两个 app 的 TOML 模型确有重复，但 daemon 本次
//! > 不动，新造结构当下只有 xiaoo 一个使用者，违反 C-2。API 层的配置降维
//! > 由 [`crate::host::SessionOptions`]（阶段 2 落地）承担——它是运行时选项
//! > 对象，不是配置文件模型；TOML → SessionOptions 的翻译留在各 app。
//! > 文件格式收敛推迟到 daemon 拆分后（§7）。

pub mod builtin_agent_roles;
pub mod cron;
pub mod lsp;
pub mod mcp;

// ---- skills 配置件（单类型，无子模块）----
#[doc(inline)]
pub use skill::SkillsConfig;
