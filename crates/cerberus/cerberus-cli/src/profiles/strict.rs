//! Workspace-write network-off profile.
//!
//! Maximum isolation for untrusted code.

use cerberus_core::{EnvironmentConfig, NamespaceConfig, PathGroups, Policy, ResourceLimits};

/// Create the workspace-write-network-off profile.
pub fn create() -> Policy {
    Policy {
        path_groups: PathGroups::strict(),
        custom_paths: vec![],
        namespaces: NamespaceConfig::full(),
        resources: ResourceLimits {
            timeout_secs: 30,
            max_memory_bytes: Some(256 * 1024 * 1024),
            max_processes: Some(50),
        },
        environment: strict_environment(),
        network_policy: None,
        landlock_optional: false,
        mount_isolation_fallback: false,
    }
}

fn strict_environment() -> EnvironmentConfig {
    EnvironmentConfig {
        whitelist: vec!["PATH".to_string(), "LANG".to_string(), "TERM".to_string()],
    }
}
