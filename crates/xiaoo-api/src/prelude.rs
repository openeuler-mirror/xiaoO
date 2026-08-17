//! 门面一站式 import。
//!
//! 【L0 门面层】调用方 `use xiaoo_api::prelude::*;` 即可写完一个完整对话程序
//! （见 `crate` 根文档的最小用例）。
//!
//! ## 已落地（阶段 2.1 / 2.2）
//!
//! - `LocalSessionHost` / `LocalSessionHostBuilder` / `HostBuildError` / `SecretsInit`
//! - `Session` / `SessionOptions` / `LlmOptions`
//! - 签名直接需要的 `AppTurnResult` / `SessionRecord` / `SessionServiceError`
//!
//! ## 阶段 2.3 待落地
//!
//! - `TurnHandle` / `TurnEvent` / `TurnOptions` / `InteractionResponder`
//!
//! 一旦阶段 2.3 落地，本 prelude 的导出面即达到 §3.2 设计的 ≤ 15 个名字目标。

pub use crate::host::{
    HostBuildError, LocalSessionHost, LocalSessionHostBuilder, LlmOptions, SecretsInit,
    Session, SessionOptions, SessionOptionsError,
};

// 签名直接需要的支撑类型（来自 L1 层，调用方在写完整程序时会触达）。
pub use crate::session::{AppTurnResult, SessionServiceError};
pub use crate::session::SessionRecord;
