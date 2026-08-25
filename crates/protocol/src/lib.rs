//! xiaoo wire protocol assets.
//!
//! This crate owns the wire-facing types that flow across the xiaoo ↔ daemon
//! boundary: the strongly-typed [`sse::RuntimeSseEvent`] model (with the
//! [`plan::TodoSnapshotItem`] payload it references) and the [`wire`] request
//! types that clients send over the `/api/v1/runtimes/*` HTTP surface.  It
//! depends only on `serde`/`serde_json` and `agent-types`; it has no
//! dependency on any `apps/*` crate.
//!
//! # Protocol discipline
//!
//! Every serde representation in this crate is a wire contract.  Any field
//! change (rename, type change, added/removed field, default flip) must stay
//! byte-for-byte compatible with the daemon's serialization and is treated as
//! a protocol change coordinated with the daemon repository.  The frozen JSON
//! baselines under `sse/tests.rs` and `wire/tests.rs` are the regression
//! guardrail: they pin the exact serialized shape and must stay green.  When
//! the daemon legitimately evolves the protocol, update the samples through a
//! dedicated protocol-change review, never as a drive-by.

#![deny(missing_docs)]

pub mod plan;
pub mod sse;
pub mod wire;
