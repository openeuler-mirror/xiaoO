//! /api/v1/runtimes/input 的 SSE 事件模型。
//!
//! 【L1 支撑类型层；本阶段为占位】
//!
//! 现状问题：daemon 在 `apps/serverside/src/httpserver/sse_sink.rs` 手工拼 JSON 事件，
//! xiaoo 在 `apps/endside/src/gateway_api/remote.rs` 与 `cli/entry.rs` 两处手工解析——
//! 协议存在但没有共享类型，双方靠字符串字段名隐式耦合。
//!
//! 本模块将在阶段 2 提供强类型模型 `RuntimeSseEvent`，**字段名/结构必须与 daemon
//! 现有序列化逐字段对齐**（实现时以 `sse_sink.rs` 的实际输出为唯一基准，先写兼容性
//! 测试再迁移）。事件词汇与门面 [`crate::host::TurnEvent`]（阶段 2 落地）对齐——
//! 远程 Session 实现（§7）把 `RuntimeSseEvent` 1:1 映射为 `TurnEvent`。
//!
//! 兼容性纪律：本模块类型的 serde 表示是 wire 契约，任何字段增删都要与 daemon 的
//! 实际输出核对；未知字段一律容忍（`deny_unknown_fields` 禁用）。
