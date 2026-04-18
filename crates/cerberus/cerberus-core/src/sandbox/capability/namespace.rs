//! Namespace capability types.

/// Namespace capability status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamespaceCapability {
    /// Mount namespace support.
    pub mount: bool,
    /// PID namespace support.
    pub pid: bool,
    /// Network namespace support.
    pub network: bool,
    /// User namespace support.
    pub user: bool,
}

impl NamespaceCapability {
    /// Check if any namespace isolation is available.
    pub fn any_available(&self) -> bool {
        self.mount || self.pid || self.network || self.user
    }
}
