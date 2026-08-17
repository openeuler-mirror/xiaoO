//! 用户交互契约（工具审批 / 用户提问）。
//!
//! 【L1 支撑类型层】
//!
//! - [`InteractionHandle`]（trait，偏 L2）是 Agent 内核工具审批/用户提问的
//!   唯一底层通道：`ask(&InteractionRequest) -> InteractionResponse` 阻塞等待
//!   用户应答；`has_builtin_timeout()` 告知内核是否需要外层超时；
//!   `abort_pending()` 在取消 turn 时清场。门面 [`crate::host::TurnEvent::Interaction`]
//!   + [`crate::host::InteractionResponder`]（阶段 2 落地）是它的事件流投影。
//! - [`InteractionRequest`] / [`InteractionResponse`] 是 wire 载荷：daemon 在
//!   SSE `interaction` 事件中序列化 `InteractionRequest`，xiaoo 经
//!   `POST /runtimes/interaction` 提交 `InteractionResponse`。

#[doc(inline)]
pub use agent_contracts::interaction::InteractionHandle;

#[doc(inline)]
pub use agent_types::interaction::types::{InteractionRequest, InteractionResponse, InteractionSource};
