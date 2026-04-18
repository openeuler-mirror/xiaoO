//! Policy configuration for sandboxed execution.

use super::{EnvironmentConfig, FsRule, NamespaceConfig, PathGroups, ResourceLimits};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Main policy configuration.
///
/// Combines filesystem rules, namespace configuration, resource limits,
/// and network policy into a complete sandbox configuration.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Predefined filesystem path groups.
    #[serde(default)]
    pub path_groups: PathGroups,
    /// Custom filesystem access rules.
    #[serde(default)]
    pub custom_paths: Vec<FsRule>,
    /// Namespace isolation configuration.
    #[serde(default)]
    pub namespaces: NamespaceConfig,
    /// Resource limits.
    #[serde(default)]
    pub resources: ResourceLimits,
    /// Environment variable configuration.
    #[serde(default)]
    pub environment: EnvironmentConfig,
    /// Network policy applied when network access is allowed.
    #[serde(default)]
    pub network_policy: Option<super::NetworkPolicy>,
    /// Whether Landlock filesystem isolation is optional.
    #[serde(default)]
    pub landlock_optional: bool,
    /// Whether to use mount isolation fallback.
    #[serde(default)]
    pub mount_isolation_fallback: bool,
}

impl Policy {
    /// Check if network access is allowed.
    ///
    /// Returns true when the public policy allows network access.
    pub fn allow_network(&self) -> bool {
        self.namespaces.network
    }

    /// Create a strict policy with full isolation.
    ///
    /// - Network blocked
    /// - Full namespace isolation
    /// - Essential filesystem paths only
    pub fn strict() -> Self {
        Self {
            path_groups: PathGroups::strict(),
            custom_paths: Vec::new(),
            namespaces: NamespaceConfig::full(),
            resources: ResourceLimits::default(),
            environment: EnvironmentConfig::default(),
            network_policy: None,
            landlock_optional: false,
            mount_isolation_fallback: false,
        }
    }

    /// Create a policy with network access enabled.
    ///
    /// - Network allowed
    /// - User namespace disabled (required for network)
    pub fn with_network() -> Self {
        Self {
            path_groups: PathGroups::minimal(),
            custom_paths: Vec::new(),
            namespaces: NamespaceConfig::without_user(),
            resources: ResourceLimits::default(),
            environment: EnvironmentConfig::default(),
            network_policy: None,
            landlock_optional: false,
            mount_isolation_fallback: false,
        }
    }

    /// Create a minimal policy with basic isolation.
    ///
    /// - Only mount namespace
    /// - Minimal filesystem paths
    pub fn minimal() -> Self {
        Self {
            path_groups: PathGroups::minimal(),
            custom_paths: Vec::new(),
            namespaces: NamespaceConfig::minimal(),
            resources: ResourceLimits::default(),
            environment: EnvironmentConfig::default(),
            network_policy: None,
            landlock_optional: false,
            mount_isolation_fallback: false,
        }
    }

    /// Get all filesystem rules (path groups + custom paths).
    pub fn fs_rules(&self) -> Vec<FsRule> {
        let mut rules = self.path_groups.to_rules();
        rules.extend(self.custom_paths.clone());
        rules
    }

    /// Get the execution timeout.
    pub fn timeout(&self) -> Duration {
        self.resources.timeout()
    }

    /// Validate the policy configuration.
    pub fn validate(&self) -> Result<(), super::PolicyError> {
        if let Some(ref network_policy) = self.network_policy {
            network_policy.validate()?;
            network_policy.validate_network_access_compatibility(self.allow_network())?;
        }

        Ok(())
    }

    /// Create a new policy builder.
    pub fn builder() -> super::PolicyBuilder {
        super::PolicyBuilder::new()
    }
}

impl Default for Policy {
    fn default() -> Self {
        Self::strict()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strict_policy_defaults() {
        let policy = Policy::strict();
        assert!(!policy.allow_network());
        assert!(!policy.namespaces.network);
        assert_eq!(policy.resources.timeout_secs, 30);
        assert_eq!(policy.resources.max_memory_bytes, Some(256 * 1024 * 1024));
        assert!(!policy.fs_rules().is_empty());
    }

    #[test]
    fn test_with_network_policy() {
        let policy = Policy::with_network();
        assert!(policy.allow_network());
        assert!(policy.namespaces.network);
        assert!(!policy.namespaces.user);
        assert!(!policy
            .fs_rules()
            .iter()
            .any(|rule| rule.path == std::path::PathBuf::from("/etc")));
    }
}
