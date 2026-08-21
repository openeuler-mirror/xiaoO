//! xiaoo wire protocol assets.
//!
//! This crate owns the wire-facing types that clients use to consume the
//! daemon's SSE stream: the strongly-typed [`sse::RuntimeSseEvent`] model and
//! the [`plan::TodoSnapshotItem`] payload it references.  It depends only on
//! `serde` and `agent-types`; it has no dependency on any `apps/*` crate.
//!
//! # Scope
//!
//! v1 covers the SSE event surface recovered from the pre-refactor SDK.  The
//! wire request aliases (`RuntimeOpenRequest` / `RuntimeTurnRequest` /
//! `RuntimeCancelRequest` / `RuntimeCloseRequest` / `RuntimeDetachRequest` /
//! `RuntimeHeartbeatRequest` / `RuntimeInteractionRequest`) currently remain
//! in `xiaoo_shared::gateway` (endside's remote client already depends on
//! shared, and moving them would sink the whole session DTO family).  They
//! are registered as a second-stage "contract sinking" item to migrate into
//! this crate.

#![deny(missing_docs)]

pub mod plan;
pub mod sse;
