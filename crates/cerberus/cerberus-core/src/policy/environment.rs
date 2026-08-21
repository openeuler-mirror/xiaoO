//! Environment variable handling types.
//!
//! This module provides types for controlling which environment
//! variables are passed to sandboxed processes.

use serde::{Deserialize, Serialize};

fn default_env_whitelist() -> Vec<String> {
    vec![
        "PATH".to_string(),
        "LANG".to_string(),
        "HOME".to_string(),
        "USER".to_string(),
        "HTTP_PROXY".to_string(),
        "HTTPS_PROXY".to_string(),
        "http_proxy".to_string(),
        "https_proxy".to_string(),
        "ALL_PROXY".to_string(),
        "all_proxy".to_string(),
        "NO_PROXY".to_string(),
        "no_proxy".to_string(),
    ]
}

/// Environment variable configuration.
///
/// Controls which environment variables are passed to the sandboxed process.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentConfig {
    /// List of environment variable names to pass through.
    #[serde(default = "default_env_whitelist")]
    pub whitelist: Vec<String>,
}

impl Default for EnvironmentConfig {
    fn default() -> Self {
        Self {
            whitelist: default_env_whitelist(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_environment_config_default() {
        let config = EnvironmentConfig::default();
        assert!(config.whitelist.contains(&"PATH".to_string()));
        assert!(config.whitelist.contains(&"HOME".to_string()));
        assert!(config.whitelist.contains(&"HTTP_PROXY".to_string()));
    }

    #[test]
    fn test_environment_config_custom_whitelist() {
        let config = EnvironmentConfig {
            whitelist: vec!["PATH".to_string(), "LANG".to_string()],
        };
        assert_eq!(config.whitelist.len(), 2);
    }
}
