//! 门面层：本地会话宿主、会话句柄、一轮对话与事件流。
//!
//! 【L0 门面层】
//!
//! ```text
//! LocalSessionHostBuilder ──build()──▶ LocalSessionHost（进程级资源）
//!                                           │ open_session(SessionOptions)
//!                                           ▼
//!                                        Session（会话句柄）
//!                                           │ run_turn(text) / send(text)
//!                                           ▼
//!                                        TurnHandle（事件流 + 取消 + 追加输入 + 结果）
//! ```
//!
//! 90+ 个底层符号被压缩到这条主线之后：调用方按
//! `builder().build() → open_session() → run_turn() → close() → shutdown()`
//! 的顺序走完全生命周期，每一步只有一个明显的入口。
//!
//! ## 阶段 2.1 已落地
//!
//! - [`options::SessionOptions`] / [`options::LlmOptions`]：调用方仅声明"差异"，其余
//!   由 [`options::SessionOptions::derive`] 派生为 `(SessionOpenRequest,
//!   HostedSessionRuntimeConfig)`。派生内部调用 4 组 helper。
//! - 行为快照测试覆盖 skills / context-window 两份重复实现的合并；派生结果与
//!   `apps/endside/src/cli/entry.rs:956-1038` 的现有组装逐字段对照。
//!
//! ## 阶段 2.2 已落地
//!
//! - [`local_session_host::LocalSessionHost`] / [`local_session_host::LocalSessionHostBuilder`]
//!   / [`local_session_host::HostBuildError`] / [`local_session_host::SecretsInit`]：
//!   进程级资源容器与 builder。`build()` 内部初始化 secrets、连接 memory automation、
//!   构造 lifecycle control plane。`open_session` / `shutdown` 主入口落地。
//! - [`session::Session`]：会话句柄。`id` / `send` / `update_options` / `export` /
//!   `close` / `run_turn_raw` 方法落地。
//!
//! ## 阶段 2.3 已落地
//!
//! - [`turn_handle::TurnHandle`] / [`turn_handle::TurnEvent`] /
//!   [`turn_handle::InteractionResponder`] / [`turn_handle::TurnOptions`]：
//!   一轮对话的流式句柄与事件流。事件词汇与远程 SSE 模型对齐。
//! - [`session::Session::run_turn`] / [`session::Session::run_turn_with`]：流式入口。
//!   内部 sink 适配器（[`sink_adapters`]）把 6 个 sink trait 适配成 `TurnEvent` 通道，
//!   提炼自 endside `session_sink.rs` / `session_interaction.rs` / `cli/mod.rs:50-107`。

pub mod derive;
pub mod local_session_host;
pub mod options;
pub mod session;
pub mod sink_adapters;
pub mod turn_handle;

pub use local_session_host::{
    HostBuildError, LocalSessionHost, LocalSessionHostBuilder, SecretsInit,
};
pub use options::{LlmOptions, SessionOptions, SkillsSection};
pub use session::Session;
pub use turn_handle::{InteractionResponder, TurnEvent, TurnHandle, TurnOptions};

#[cfg(test)]
mod tests;
