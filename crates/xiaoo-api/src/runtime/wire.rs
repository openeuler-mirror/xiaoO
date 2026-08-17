//! /api/v1/runtimes/* 的 wire 协议类型（HTTP client ↔ daemon HTTP server）。
//!
//! 【L1 支撑类型层】xiaoo（client）与 daemon（server）之间唯一的序列化
//! 协议面。serde 表示属协议契约，任何字段增删都需两侧协同。re-export
//! 不产生新类型——这些类型与 `xiaoo_shared` 中的定义是同一个类型，daemon
//! 端 `apps/serverside/src/httpserver/router.rs` 与 xiaoo 端
//! `apps/endside/src/gateway_api/remote.rs` 共用同一组类型实例。
//!
//! SSE 事件流的强类型模型见 [`crate::sse`]。

// ---- /runtimes/* 主接口的 wire 请求别名（与 Session*Request 同源，wire 化命名）----
#[doc(inline)]
pub use xiaoo_shared::gateway::{
    RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, RuntimeTurnRequest,
};

// ---- 快照/评测/文件操作的 wire 请求-响应类型 ----
#[doc(inline)]
pub use xiaoo_shared::{
    RuntimeCheckoutRequest, RuntimeCheckoutResult, RuntimeCheckpointRequest,
    RuntimeCheckpointResult, RuntimeCheckpointSnapshotDeleteRequest,
    RuntimeCheckpointSnapshotDeleteResult, RuntimeExecRequest, RuntimeExecResult,
    RuntimePauseRequest, RuntimePauseResult, RuntimeReadFileRequest, RuntimeReadFileResult,
    RuntimeRecord, RuntimeResumeRequest, RuntimeResumeResult, RuntimeWriteFileRequest,
    RuntimeWriteFileResult,
};
