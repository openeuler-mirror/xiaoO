//! xiaoo agent runtime SDK.
//!
//! The SDK exposes the decision loop and its operation-plane contracts.  The
//! caller owns conversation state and injects any sandbox instance; session
//! stores, leases, backend managers, and gateway bootstrapping are not part of
//! this crate.

#![deny(missing_docs)]

pub mod backend;
pub mod chat;
pub mod events;
pub mod interaction;
pub mod llm;
pub mod runtime;
pub mod skills;
pub mod tools;

pub mod prelude;
