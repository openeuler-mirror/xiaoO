//! 记忆自动化契约。
//!
//! 【L1 支撑类型层】
//!
//! ## 用法要点
//!
//! `McpMemoryAutomation::connect(config, mcp_servers) ->
//! Result<Option<Arc<dyn TurnMemoryAutomation>>, _>`：config 未启用时返回
//! `None`。宿主退出前必须 `close()`（daemon 限时 5s，门面 `shutdown` 已内置
//! 同样限时）。`subscribe_health()` 返回 `watch::Receiver<MemoryAutomationHealth>`，
//! 门面经 `host.memory_health()` 透出。
//!
//! 常规调用方经 [`crate::host::LocalSessionHostBuilder::memory_automation_config`]
//! （阶段 2 落地）即可，不直接触碰本模块。

#[doc(inline)]
pub use xiaoo_shared::gateway::memory_automation::{
    CompletedTurnIngest, McpMemoryAutomation, MemoryAutomationConfig, MemoryAutomationError,
    MemoryAutomationHealth, RecallMemory, TurnMemoryAutomation, TurnMemoryContext,
};
