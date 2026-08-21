//! Tool registry vocabulary for the runtime SDK.
//!
//! The builder's [`crate::runtime::RuntimeBuilder::tool_registry`] setter and
//! the [`crate::runtime::Runtime::visible_tools`] accessor reference these
//! types; they are re-exported here so callers depend only on `xiaoo_api` when
//! assembling a runtime.

#[doc(inline)]
pub use agent_contracts::{ToolRegistry, ToolSpecView};

#[doc(inline)]
pub use tool::{EmptyToolRegistry, ToolSpecSnapshot};
