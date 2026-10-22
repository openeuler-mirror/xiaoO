//! User interaction contract (tool approval / user questions).
//!
//! [`InteractionHandle`] is the runtime channel for approvals and questions.
//! A handle supplied on [`crate::runtime::RuntimeInput`] is also attached to
//! the injected operation backend for sandbox permission prompts. This module
//! also re-exports the no-op default implementation for unattended scenarios.

#[doc(inline)]
pub use agent_contracts::interaction::InteractionHandle;

#[doc(inline)]
pub use agent_types::interaction::types::{
    InteractionRequest, InteractionResponse, InteractionSource,
};

// ---- Default implementation ----

/// Default interaction handle that refuses every prompt (Confirm → not
/// allowed, TextInput/Choice → `None`); the standard implementation for
/// unattended turns where no human is available to answer.
#[doc(inline)]
pub use xiaoo_core::NoopInteractionHandle;
