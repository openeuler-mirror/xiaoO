//! Loop / tool 事件类型与 sink 契约。
//!
//! 【L1 支撑类型层】`LoopEventSink` / `ToolEventSink` 是 Agent 内核回调的契约
//! 接口；`ToolLifecycleEvent` / `ToolResultEvent` / `LoopEndSummary` 是事件负载
//! 类型，同时也是 SSE `Done` / `tool` / `tool_result` 事件的 wire 载荷。本模块
//! 同时提供丢弃事件的 Noop 默认实现，用于无需采集事件的场景。
//!
//! `LoopEventSink` receives progressive snapshots rather than text deltas;
//! consumers that need deltas can diff adjacent snapshots.

// ---- sink 契约 trait ----
#[doc(inline)]
pub use agent_contracts::events::{LoopEventSink, ToolEventSink};

// ---- 事件负载类型 ----
#[doc(inline)]
pub use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};

// ---- 构建默认件 ----

/// Default `ToolEventSink` that discards every event; the standard
/// implementation when a caller does not need to collect tool lifecycle
/// events.
#[doc(inline)]
pub use xiaoo_core::NoopToolEventSink;
