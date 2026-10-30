//! Messages and common small types.
//!
//! These are the message, identity, and configuration values consumed by the
//! runtime loop. They carry no session-service lifecycle semantics.

// ---- Messages and roles ----
#[doc(inline)]
pub use llm_client::{ChatMessage, ContentBlock, MessageRole};

// ---- Reasoning effort ----
#[doc(inline)]
pub use agent_types::ReasoningEffort;

// ---- Slash command metadata ----
#[doc(inline)]
pub use agent_types::chat::CommandContext;

// ---- Identifiers ----
#[doc(inline)]
pub use agent_types::common::ids::{AgentId, ToolName};

// ---- Hook actions and config ----
#[doc(inline)]
pub use agent_types::hook::{HookAction, HookerDefaultMode, HookerRegistryConfig};

// ---- Feature flags and token budget ----
#[doc(inline)]
pub use agent_types::context::{FeatureFlags, TokenBudgetConfig};

// ---- max_tokens vs context-window policy (shared by the TUI turn gate
// and the daemon's startup/override validation) ----
#[doc(inline)]
pub use agent_types::context::{validate_max_tokens_against_window, MAX_REASONABLE_OUTPUT_RATIO};
