//! 门面一站式 import。
//!
//! 【L0 门面层】调用方 `use xiaoo_api::prelude::*;` 即可写完一个完整对话程序
//! （见 `crate` 根文档的最小用例）。
//!
//! ## 已落地（阶段 2.1 / 2.2 / 2.3）
//!
//! - `LocalSessionHost` / `LocalSessionHostBuilder` / `HostBuildError` / `SecretsInit`
//! - `Session` / `SessionOptions` / `LlmOptions`
//! - `TurnHandle` / `TurnEvent` / `TurnOptions` / `InteractionResponder`
//! - 签名直接需要的 `AppTurnResult` / `SessionRecord` / `SessionServiceError`
//!
//! 导出面共 14 个名字，达到 ≤ 15 个名字的设计目标
//! （占位类型 `SessionOptionsError` 已删除，回落 14 名）。

pub use crate::host::{
    HostBuildError, InteractionResponder, LocalSessionHost, LocalSessionHostBuilder, LlmOptions,
    SecretsInit, Session, SessionOptions, TurnEvent, TurnHandle, TurnOptions,
};

// 签名直接需要的支撑类型（来自 L1 层，调用方在写完整程序时会触达）。
pub use crate::session::{AppTurnResult, SessionServiceError};
pub use crate::session::SessionRecord;
