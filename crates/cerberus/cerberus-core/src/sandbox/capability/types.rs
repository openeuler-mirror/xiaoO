//! Sandbox capabilities aggregate type.

use super::level::{CapabilityLevel, EnforcementLevel};
use super::namespace::NamespaceCapability;

/// Detected sandbox capabilities for the current platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxCapabilities {
    /// Landlock LSM capability level.
    pub landlock: CapabilityLevel,
    /// Seccomp BPF capability level.
    pub seccomp: CapabilityLevel,
    /// Namespace isolation capability.
    pub namespaces: NamespaceCapability,
    /// Mount isolation fallback capability.
    pub mount_isolation: CapabilityLevel,
    /// Overall enforcement level for the current platform.
    pub enforcement_level: EnforcementLevel,
}
