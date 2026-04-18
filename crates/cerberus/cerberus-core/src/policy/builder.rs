//! Policy builder with fluent API.
//!
//! This module provides a builder pattern for constructing
//! policy configurations.

use std::time::Duration;

use super::environment::EnvironmentConfig;
use super::filesystem::{FsPermission, FsRule, PathGroups};
use super::network::NetworkPolicy;
use super::process::{NamespaceConfig, ResourceLimits};
use super::Policy;

/// Builder for creating policy configurations.
///
/// Provides a fluent API for constructing policies with
/// custom filesystem access, network rules, and resource limits.
pub struct PolicyBuilder {
    path_groups: PathGroups,
    custom_paths: Vec<FsRule>,
    namespaces: NamespaceConfig,
    resources: ResourceLimits,
    environment: EnvironmentConfig,
    network_policy: Option<NetworkPolicy>,
    landlock_optional: bool,
    mount_isolation_fallback: bool,
}

impl PolicyBuilder {
    /// Create a new builder with strict defaults.
    pub fn new() -> Self {
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

    /// Create a builder with strict policy preset.
    pub fn strict() -> Self {
        let policy = Policy::strict();
        Self::from_policy(policy)
    }

    /// Create a builder with network-enabled policy preset.
    pub fn with_network() -> Self {
        let policy = Policy::with_network();
        Self::from_policy(policy)
    }

    /// Create a builder with minimal policy preset.
    pub fn minimal() -> Self {
        let policy = Policy::minimal();
        Self::from_policy(policy)
    }

    fn from_policy(policy: Policy) -> Self {
        Self {
            path_groups: policy.path_groups,
            custom_paths: policy.custom_paths,
            namespaces: policy.namespaces,
            resources: policy.resources,
            environment: policy.environment,
            network_policy: policy.network_policy,
            landlock_optional: policy.landlock_optional,
            mount_isolation_fallback: policy.mount_isolation_fallback,
        }
    }

    /// Set the predefined path groups.
    pub fn path_groups(mut self, groups: PathGroups) -> Self {
        self.path_groups = groups;
        self
    }

    /// Add read-only access to a path.
    pub fn allow_read(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.custom_paths.push(FsRule {
            path: path.into(),
            permission: FsPermission::ReadOnly,
        });
        self
    }

    /// Add read-write access to a path.
    pub fn allow_write(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.custom_paths.push(FsRule {
            path: path.into(),
            permission: FsPermission::ReadWrite,
        });
        self
    }

    /// Add read-execute access to a path.
    pub fn allow_execute(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.custom_paths.push(FsRule {
            path: path.into(),
            permission: FsPermission::ReadExecute,
        });
        self
    }

    /// Add read-write-execute access to a path.
    pub fn allow_read_write_execute(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.custom_paths.push(FsRule {
            path: path.into(),
            permission: FsPermission::ReadWriteExecute,
        });
        self
    }

    /// Set network access (true = allow network, false = block).
    pub fn network(mut self, allow: bool) -> Self {
        self.namespaces.network = allow;
        self
    }

    /// Set timeout in seconds.
    pub fn timeout_secs(mut self, secs: u64) -> Self {
        self.resources.timeout_secs = secs;
        self
    }

    /// Set timeout as a Duration.
    pub fn timeout(mut self, duration: Duration) -> Self {
        self.resources.timeout_secs = duration.as_secs();
        self
    }

    /// Set maximum memory in bytes.
    pub fn max_memory(mut self, bytes: u64) -> Self {
        self.resources.max_memory_bytes = Some(bytes);
        self
    }

    /// Set maximum number of processes.
    pub fn max_processes(mut self, count: u32) -> Self {
        self.resources.max_processes = Some(count);
        self
    }

    /// Set namespace configuration.
    pub fn namespaces(mut self, config: NamespaceConfig) -> Self {
        self.namespaces = config;
        self
    }

    /// Set environment variable whitelist.
    pub fn env_whitelist(mut self, vars: Vec<String>) -> Self {
        self.environment.whitelist = vars;
        self
    }

    /// Set whether Landlock is optional (fallback to no filesystem isolation).
    pub fn landlock_optional(mut self, optional: bool) -> Self {
        self.landlock_optional = optional;
        self
    }

    /// Set whether mount isolation fallback is enabled.
    pub fn mount_isolation_fallback(mut self, enabled: bool) -> Self {
        self.mount_isolation_fallback = enabled;
        self
    }

    /// Set the network policy.
    pub fn network_policy(mut self, policy: NetworkPolicy) -> Self {
        self.network_policy = Some(policy);
        self
    }

    /// Build the final policy.
    pub fn build(self) -> Policy {
        Policy {
            path_groups: self.path_groups,
            custom_paths: self.custom_paths,
            namespaces: self.namespaces,
            resources: self.resources,
            environment: self.environment,
            network_policy: self.network_policy,
            landlock_optional: self.landlock_optional,
            mount_isolation_fallback: self.mount_isolation_fallback,
        }
    }
}

impl Default for PolicyBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_builder_with_custom_paths() {
        let policy = Policy::builder()
            .allow_read("/home/user")
            .allow_write("/data")
            .allow_execute("/opt/bin")
            .allow_read_write_execute("/workspace/bin")
            .build();

        let custom_paths: Vec<&str> = policy
            .custom_paths
            .iter()
            .map(|r| r.path.to_str().unwrap())
            .collect();
        assert!(custom_paths.contains(&"/home/user"));
        assert!(custom_paths.contains(&"/data"));
        assert!(custom_paths.contains(&"/opt/bin"));
        assert!(custom_paths.contains(&"/workspace/bin"));
    }

    #[test]
    fn test_builder_network_enables_paths() {
        let policy = Policy::builder().network(true).build();
        assert!(policy.allow_network());
        assert!(policy.namespaces.user);
    }

    #[test]
    fn test_max_processes_builder() {
        let policy = Policy::builder().max_processes(10).build();
        assert_eq!(policy.resources.max_processes, Some(10));
    }

    #[test]
    fn test_builder_default() {
        let builder = PolicyBuilder::default();
        let policy = builder.build();
        assert!(!policy.allow_network());
    }

    #[test]
    fn test_builder_network_false_blocks_public_network_access() {
        let policy = Policy::builder().network(false).build();

        assert!(!policy.allow_network());
        assert!(!policy.namespaces.network);
    }
}
