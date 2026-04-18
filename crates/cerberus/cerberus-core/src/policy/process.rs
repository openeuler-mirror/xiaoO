//! Process isolation and resource limit types.
//!
//! This module provides types for configuring namespace isolation
//! and resource limits for sandboxed processes.

use serde::{Deserialize, Deserializer, Serialize};
use std::time::Duration;

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_timeout() -> u64 {
    30
}

/// Deserialize an optional u64, normalizing 0 to None (unlimited).
fn deserialize_optional_limit_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<u64>::deserialize(deserializer)?;
    // Normalize 0 to None (unlimited) to prevent dangerous setrlimit(0)
    Ok(opt.filter(|&v| v > 0))
}

/// Deserialize an optional u32, normalizing 0 to None (unlimited).
fn deserialize_optional_limit_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<u32>::deserialize(deserializer)?;
    // Normalize 0 to None (unlimited) to prevent dangerous setrlimit(0)
    Ok(opt.filter(|&v| v > 0))
}

/// Namespace isolation configuration.
///
/// Controls which Linux namespaces are used to isolate the process.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct NamespaceConfig {
    /// Isolate mount namespace (separate filesystem view).
    #[serde(default = "default_true")]
    pub mount: bool,
    /// Isolate PID namespace (separate process ID space).
    #[serde(default = "default_true")]
    pub pid: bool,
    /// Allow network access.
    #[serde(default = "default_false")]
    pub network: bool,
    /// Isolate user namespace (unprivileged operation).
    #[serde(default = "default_true")]
    pub user: bool,
}

impl NamespaceConfig {
    /// Full isolation: all namespaces enabled.
    pub fn full() -> Self {
        Self {
            mount: true,
            pid: true,
            network: false,
            user: true,
        }
    }

    /// Minimal isolation: only mount namespace.
    pub fn minimal() -> Self {
        Self {
            mount: true,
            pid: false,
            network: true,
            user: false,
        }
    }

    /// Isolation without user namespace (for network access).
    pub fn without_user() -> Self {
        Self {
            mount: true,
            pid: true,
            network: true,
            user: false,
        }
    }

    pub(crate) fn linux_runtime_plan(&self) -> LinuxNamespacePlan {
        LinuxNamespacePlan {
            mount: self.mount,
            pid: self.pid,
            user: self.user,
            isolate_network: !self.network,
        }
    }

    pub(crate) fn requires_linux_namespaces(&self) -> bool {
        let plan = self.linux_runtime_plan();
        plan.mount || plan.pid || plan.user || plan.isolate_network
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinuxNamespacePlan {
    pub mount: bool,
    pub pid: bool,
    pub user: bool,
    pub isolate_network: bool,
}

impl Default for NamespaceConfig {
    fn default() -> Self {
        Self::full()
    }
}

/// Resource limits for sandboxed processes.
///
/// Controls execution time, memory, and process count limits.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceLimits {
    /// Maximum execution time in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Maximum memory in bytes (None = unlimited, 0 is normalized to None).
    #[serde(
        default,
        deserialize_with = "deserialize_optional_limit_u64",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_memory_bytes: Option<u64>,
    /// Maximum number of processes (None = unlimited, 0 is normalized to None).
    #[serde(
        default,
        deserialize_with = "deserialize_optional_limit_u32",
        skip_serializing_if = "Option::is_none"
    )]
    pub max_processes: Option<u32>,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            max_memory_bytes: Some(256 * 1024 * 1024),
            max_processes: None,
        }
    }
}

impl ResourceLimits {
    /// Get the timeout as a Duration.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_namespace_config_full() {
        let config = NamespaceConfig::full();
        assert!(config.mount);
        assert!(config.pid);
        assert!(!config.network);
        assert!(config.user);
    }

    #[test]
    fn test_namespace_config_minimal() {
        let config = NamespaceConfig::minimal();
        assert!(config.mount);
        assert!(!config.pid);
        assert!(config.network);
        assert!(!config.user);
    }

    #[test]
    fn test_namespace_config_without_user() {
        let config = NamespaceConfig::without_user();
        assert!(config.mount);
        assert!(config.pid);
        assert!(config.network);
        assert!(!config.user);
    }

    #[test]
    fn test_linux_runtime_plan_blocks_network_only_when_public_value_is_false() {
        let blocked = NamespaceConfig::full().linux_runtime_plan();
        let allowed = NamespaceConfig::without_user().linux_runtime_plan();

        assert!(blocked.isolate_network);
        assert!(!allowed.isolate_network);
    }

    #[test]
    fn test_resource_limits_default() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.timeout_secs, 30);
        assert_eq!(limits.max_memory_bytes, Some(256 * 1024 * 1024));
        assert_eq!(limits.max_processes, None);
    }

    #[test]
    fn test_resource_limits_timeout() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.timeout(), Duration::from_secs(30));
    }

    #[test]
    fn test_resource_limits_zero_normalized_to_none() {
        let toml = r#"
            timeout_secs = 30
            max_memory_bytes = 0
            max_processes = 0
        "#;
        let limits: ResourceLimits = toml::from_str(toml).unwrap();
        assert_eq!(limits.max_memory_bytes, None);
        assert_eq!(limits.max_processes, None);
    }

    #[test]
    fn test_resource_limits_positive_values_preserved() {
        let toml = r#"
            timeout_secs = 60
            max_memory_bytes = 536870912
            max_processes = 100
        "#;
        let limits: ResourceLimits = toml::from_str(toml).unwrap();
        assert_eq!(limits.max_memory_bytes, Some(536870912));
        assert_eq!(limits.max_processes, Some(100));
    }

    #[test]
    fn test_resource_limits_omitted_fields_are_none() {
        let toml = r#"
            timeout_secs = 30
        "#;
        let limits: ResourceLimits = toml::from_str(toml).unwrap();
        assert_eq!(limits.max_memory_bytes, None);
        assert_eq!(limits.max_processes, None);
    }
}
