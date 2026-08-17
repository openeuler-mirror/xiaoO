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
//!   HostedSessionRuntimeConfig)`（§3.3.3）。派生内部调用 4 组 helper（§3.3.9）。
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
//!   `close` / `run_turn_raw` 方法落地；`run_turn` / `run_turn_with` 在阶段 2.3
//!   （`TurnHandle`）落地。

pub mod derive;
pub mod local_session_host;
pub mod options;
pub mod session;

pub use local_session_host::{
    HostBuildError, LocalSessionHost, LocalSessionHostBuilder, SecretsInit,
};
pub use options::{LlmOptions, SessionOptions, SessionOptionsError, SkillsSection};
pub use session::Session;

#[cfg(test)]
mod tests;
