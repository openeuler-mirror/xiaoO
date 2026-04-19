//! Workspace-write network-on profile.
//!
//! Basic isolation for trusted operations.

use cerberus_core::{EnvironmentConfig, NamespaceConfig, PathGroups, Policy, ResourceLimits};

/// Create the workspace-write-network-on profile.
pub fn create() -> Policy {
    let mut path_groups = PathGroups::minimal();
    path_groups.wsl_paths = true;

    Policy {
        path_groups,
        custom_paths: super::common_network_client_paths(),
        namespaces: NamespaceConfig::minimal(),
        resources: ResourceLimits {
            timeout_secs: 120,
            max_memory_bytes: Some(1024 * 1024 * 1024),
            max_processes: None,
        },
        environment: minimal_environment(),
        network_policy: None,
        landlock_optional: true,
        mount_isolation_fallback: true,
    }
}

fn minimal_environment() -> EnvironmentConfig {
    EnvironmentConfig {
        whitelist: vec![
            "PATH".to_string(),
            "LANG".to_string(),
            "HOME".to_string(),
            "USER".to_string(),
            "TERM".to_string(),
            "SHELL".to_string(),
            "PWD".to_string(),
        ],
    }
}
