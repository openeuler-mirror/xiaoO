//! 消息与通用小类型。
//!
//! 【L1 支撑类型层】
//!
//! These are the message, identity, and configuration values consumed by the
//! runtime loop.  They carry no session-service lifecycle semantics.

// ---- 消息与角色 ----
#[doc(inline)]
pub use llm_client::{ChatMessage, ContentBlock, MessageRole};

// ---- 推理力度 ----
#[doc(inline)]
pub use agent_types::ReasoningEffort;

// ---- slash 命令元数据 ----
#[doc(inline)]
pub use agent_types::chat::CommandContext;

// ---- 标识符 ----
#[doc(inline)]
pub use agent_types::common::ids::{AgentId, ToolName};

// ---- hook 动作与配置 ----
#[doc(inline)]
pub use agent_types::hook::{HookAction, HookerDefaultMode, HookerRegistryConfig};

// ---- feature flags 与 token budget ----
#[doc(inline)]
pub use agent_types::context::{FeatureFlags, TokenBudgetConfig};
