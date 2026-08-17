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
//! ## 阶段 2.2 待落地
//!
//! - `LocalSessionHost` / `LocalSessionHostBuilder` / `HostBuildError`
//! - `Session` / `TurnHandle` / `TurnEvent` / `TurnOptions` / `InteractionResponder`

pub mod derive;
pub mod options;

pub use options::{LlmOptions, SessionOptions, SessionOptionsError, SkillsSection};

#[cfg(test)]
mod tests;
