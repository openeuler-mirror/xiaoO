//! 消息与通用小类型。
//!
//! 【L1 支撑类型层】
//!
//! ## ChatMessage / ContentBlock 的双重身份
//!
//! [`ChatMessage`] / [`ContentBlock`] 同时是**会话历史的存储模型**
//! （`SessionRecord.loop_state.messages`）与 **SSE `Done` 事件的 wire 载荷**
//! （daemon 在 `sse_sink.rs` 终止事件里序列化 `Vec<ChatMessage>`）。其 serde
//! 表示属协议面，不可轻改。
//!
//! ## hook 配置与"聊天旁路"语义
//!
//! [`HookAction`] / [`HookerRegistryConfig`] / [`HookerDefaultMode`] 对 xiaoo
//! 而言是"聊天旁路"——`AppTurnResult.hook_actions` 由调用方决定是否执行
//! （保持 daemon / TUI 各自的执行策略空间）。
//!
//! ## feature_flags / token_budget
//!
//! [`FeatureFlags`] / [`TokenBudgetConfig`] 控制 Agent 内核行为；门面经
//! [`crate::host::SessionOptions`]（阶段 2 落地）传入，缺省值取类型 `Default`。

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
