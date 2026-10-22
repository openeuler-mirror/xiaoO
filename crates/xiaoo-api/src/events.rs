//! Loop / tool event types and sink contracts.
//!
//! `LoopEventSink` / `ToolEventSink` are the contract interfaces for Agent
//! kernel callbacks; `ToolLifecycleEvent` / `ToolResultEvent` /
//! `LoopEndSummary` are the event payload types, which also serve as the
//! wire payloads of the SSE `Done` / `tool` / `tool_result` events. This
//! module also provides the no-op default implementation that discards
//! events for scenarios that do not need to collect them.
//!
//! `LoopEventSink` receives progressive snapshots rather than text deltas;
//! consumers that need deltas can diff adjacent snapshots.

// ---- Sink contract traits ----
#[doc(inline)]
pub use agent_contracts::events::{LoopEventSink, ToolEventSink};

// ---- Event payload types ----
#[doc(inline)]
pub use agent_types::events::{LoopEndSummary, ToolLifecycleEvent, ToolResultEvent};

// ---- Default implementation ----

/// Default `ToolEventSink` that discards every event; the standard
/// implementation when a caller does not need to collect tool lifecycle
/// events.
#[doc(inline)]
pub use xiaoo_core::NoopToolEventSink;
