//! Policy configuration for sandboxed execution.
//!
//! This module provides types for configuring security policies
//! for sandboxed command execution. Policies control filesystem
//! access, network permissions, resource limits, and isolation.
//!
//! # Example
//!
//! ```rust
//! use cerberus_core::policy::{Policy, PolicyBuilder};
//!
//! // Create a strict policy
//! let policy = Policy::strict();
//!
//! // Or build a custom policy
//! let custom = Policy::builder()
//!     .network(true)
//!     .timeout_secs(60)
//!     .allow_read("/data")
//!     .build();
//! ```

mod builder;
mod environment;
mod filesystem;
mod network;
mod policy;
mod process;
mod serde;
mod validation;

pub use builder::PolicyBuilder;
pub use environment::EnvironmentConfig;
pub use filesystem::{FsPermission, FsRule, PathGroups};
pub use network::{NetworkAction, NetworkPolicy, NetworkPolicyMode, NetworkRule, PortRange};
pub use policy::Policy;
pub use process::{NamespaceConfig, ResourceLimits};
pub use validation::PolicyError;
