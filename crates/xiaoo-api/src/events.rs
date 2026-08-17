//! Loop / tool 事件类型与 sink 契约。
//!
//! 【L1 支撑类型层】`LoopEventSink` / `ToolEventSink` 是 Agent 内核回调的契约
//! 接口；`ToolLifecycleEvent` / `ToolResultEvent` / `LoopEndSummary` 是事件负载
//! 类型，同时也是 SSE `Done` / `tool` / `tool_result` 事件的 wire 载荷。
//!
//! ## 渐进快照语义
//!
//! `LoopEventSink` 的实现方收到的是**渐进快照**而非增量：`on_assistant_message`
//! 给出当前完整消息体，`on_tool_result` 给出该工具调用的最终结果。需要"增量"
//! 的渲染方应自行 diff 相邻快照；门面 [`crate::host::TurnEvent`]（阶段 2 落地）
//! 的 `Text` 增量化正是把这份 diff 收进 API 内部。
//!
//! > sink trait 本体偏【L2】：门面用户只见 `TurnEvent`；保留导出供测试替身与
//! > advanced 装配（[`crate::session::SessionService::run_turn_with_interaction`]）。

// ---- sink 契约 trait ----
#[doc(inline)]
pub use agent_contracts::events::{LoopEventSink, ToolEventSink};

// ---- 事件负载类型 ----
#[doc(inline)]
pub use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};
