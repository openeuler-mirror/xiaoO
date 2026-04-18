//! Sandbox capability detection types and functions.

mod detect;
mod level;
mod namespace;
mod types;

pub use detect::{
    check_policy_runtime_requirements, check_strict_policy_enforceable, detect_capabilities,
};
pub use level::{CapabilityLevel, EnforcementLevel};
pub use namespace::NamespaceCapability;
pub use types::SandboxCapabilities;

#[cfg(target_os = "linux")]
pub(crate) use detect::{
    check_namespace_capabilities, ensure_network_policy_backend_available,
    missing_requested_namespaces,
};
