//! Workspace-write network-on dev-env profile.
//!
//! Balanced security for AI coding assistants.

use cerberus_core::{EnvironmentConfig, NamespaceConfig, PathGroups, Policy, ResourceLimits};

/// Create the workspace-write-network-on-dev-env profile.
pub fn create() -> Policy {
    Policy {
        path_groups: llm_safe_path_groups(),
        custom_paths: super::common_network_client_paths(),
        namespaces: NamespaceConfig::without_user(),
        resources: ResourceLimits {
            timeout_secs: 60,
            max_memory_bytes: Some(512 * 1024 * 1024),
            max_processes: Some(100),
        },
        environment: llm_safe_environment(),
        network_policy: None,
        landlock_optional: false,
        mount_isolation_fallback: false,
    }
}

fn llm_safe_path_groups() -> PathGroups {
    PathGroups {
        system_binaries: true,
        system_libraries: true,
        temp_directories: true,
        device_files: true,
        proc_filesystem: true,
        wsl_paths: true,
    }
}

fn llm_safe_environment() -> EnvironmentConfig {
    EnvironmentConfig {
        whitelist: vec![
            "PATH".to_string(),
            "LANG".to_string(),
            "LC_ALL".to_string(),
            "HOME".to_string(),
            "USER".to_string(),
            "TERM".to_string(),
            "SHELL".to_string(),
            "PWD".to_string(),
            "EDITOR".to_string(),
            "VISUAL".to_string(),
            "RUST_BACKTRACE".to_string(),
            "RUST_LOG".to_string(),
            "CARGO_HOME".to_string(),
            "RUSTUP_HOME".to_string(),
            "NODE_PATH".to_string(),
            "NPM_CONFIG".to_string(),
            "PYTHONPATH".to_string(),
            "GOPATH".to_string(),
            "GOBIN".to_string(),
        ],
    }
}
