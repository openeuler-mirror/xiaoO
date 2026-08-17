//! # xiaoo-api
//!
//! 顶层 API crate：让"用 xiaoo 能力写程序"变得简单。
//!
//! 本 crate 由两部分组成（见 `refactor.md`）：
//!
//! 1. **门面层（L0，[`prelude`]）**：把"建宿主 → 开会话 → 跑 turn → 关会话"的完整链路
//!    封装成少量高层对象——`LocalSessionHost` / `Session` / `TurnHandle` / `TurnEvent`。
//!    调用方只面对这些对象即可完成常规工作，不感知内部装配细节。
//! 2. **支撑类型层（L1/L2）**：把两侧共用的契约/DTO/wire 协议按语义模块重新导出，作为
//!    门面签名的支撑类型与迁移期兼容层。re-export 的项与原定义是同一个类型/trait
//!    （Rust 的 `pub use` 不产生新类型），trait impl、serde 序列化全部兼容。
//!
//! ## 最小用例（L0 门面）
//!
//! ```ignore
//! use xiaoo_api::prelude::*;
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let host = LocalSessionHost::builder().build().await?;
//!     let session = host
//!         .open_session(SessionOptions::new(LlmOptions::new("openai", "qwen3-max")))
//!         .await?;
//!     let mut turn = session.run_turn("总结一下当前目录的代码结构").await?;
//!     while let Some(event) = turn.next_event().await {
//!         match event {
//!             TurnEvent::Text { delta, .. } => print!("{delta}"),
//!             _ => {}
//!         }
//!     }
//!     let result = turn.result().await?;
//!     println!("\n[tokens: {:?}]", result.total_tokens);
//!     session.close().await?;
//!     host.shutdown(std::time::Duration::from_secs(10)).await;
//!     Ok(())
//! }
//! ```
//!
//! > 门面尚未实现（阶段 2 才落地）；本阶段仅提供 L1/L2 支撑类型层与门面占位。
//!
//! ## 模块层级
//!
//! - **L0 门面层**：[`prelude`]、[`host`]（≤ 15 个公开名字）。
//! - **L1 支撑类型层**：[`session`]、[`runtime::wire`]、[`sse`]、[`events`]、[`plan`]、
//!   [`diff`]、[`interaction`]、[`chat`]、[`memory`]、[`backend`]、[`secrets`]、[`config`]。
//! - **L2 advanced 层**：[`runtime`]（装配契约）、[`llm`]、[`skills`]。
//!
//! 可见性策略与准入判据见 `refactor.md` §3.2。
// 阶段 1 占位：本 crate 的 doc 中含对阶段 2 才落地的 `crate::host::*`
// 门面类型的向前引用（[`crate::host::SessionOptions`] 等），阶段 2
// 实现 host 模块后这些链接会自动 resolve；届时删除此 allow。
#![allow(rustdoc::broken_intra_doc_links)]
#![deny(missing_docs)]

pub mod backend;
pub mod chat;
pub mod config;
pub mod diff;
pub mod events;
pub mod host;
pub mod interaction;
pub mod llm;
pub mod memory;
pub mod plan;
pub mod prelude;
pub mod runtime;
pub mod secrets;
pub mod session;
pub mod skills;
pub mod sse;
