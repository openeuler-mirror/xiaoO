//! /api/v1/runtimes/* 的 wire 协议类型（xiaoo HTTP client ↔ daemon HTTP server）。
//!
//! 【L1 支撑类型层】xiaoo（client）侧的序列化协议面。daemon（server 侧）
//! 已迁往独立代码仓；本仓保留 client 所需的请求类型。serde 表示属协议
//! 契约，任何字段增删都需与 daemon 仓协同。re-export 不产生新类型——
//! 这些类型与 `xiaoo_shared` 中的定义是同一个类型，xiaoo 端
//! `apps/endside/src/gateway_api/remote.rs` 直接使用。
//!
//! SSE 事件流的强类型模型见 [`crate::sse`]。

// ---- /runtimes/* 主接口的 wire 请求别名（与 Session*Request 同源，wire 化命名）----
#[doc(inline)]
pub use xiaoo_shared::gateway::{
    RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest, RuntimeHeartbeatRequest,
    RuntimeInteractionRequest, RuntimeOpenRequest, RuntimeTurnRequest,
};
