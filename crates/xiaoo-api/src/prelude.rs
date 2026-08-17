//! 门面一站式 import。
//!
//! 【L0 门面层；本阶段为占位】
//!
//! 阶段 2 落地后，本模块将聚合门面的 ≤ 15 个公开名字：
//!
//! - `LocalSessionHost` / `LocalSessionHostBuilder` / `HostBuildError`
//! - `Session` / `SessionOptions` / `LlmOptions`
//! - `TurnHandle` / `TurnEvent` / `TurnOptions`
//! - `InteractionResponder`
//! - 签名直接需要的 `AppTurnResult` / `SessionRecord` / `SessionServiceError`
//!
//! 调用方 `use xiaoo_api::prelude::*;` 即可写完一个完整对话程序（见
//! `crate` 根文档的最小用例）。
