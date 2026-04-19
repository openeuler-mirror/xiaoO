//! Capability level types.

/// Capability level for a sandbox feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityLevel {
    /// Feature is fully available and enforced.
    Full,
    /// Feature is partially available (degraded mode).
    Degraded,
    /// Feature is not available on this platform.
    Unavailable,
}

impl std::fmt::Display for CapabilityLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityLevel::Full => write!(f, "full"),
            CapabilityLevel::Degraded => write!(f, "degraded"),
            CapabilityLevel::Unavailable => write!(f, "unavailable"),
        }
    }
}

/// Overall enforcement level for sandbox execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnforcementLevel {
    /// Full enforcement: all requested isolation features are available.
    Enforced,
    /// Degraded enforcement: some isolation features are unavailable but fallbacks exist.
    Degraded,
    /// Unsupported: critical isolation features are unavailable.
    Unsupported,
}

impl std::fmt::Display for EnforcementLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnforcementLevel::Enforced => write!(f, "enforced"),
            EnforcementLevel::Degraded => write!(f, "degraded"),
            EnforcementLevel::Unsupported => write!(f, "unsupported"),
        }
    }
}
