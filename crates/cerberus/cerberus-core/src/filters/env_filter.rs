//! Environment variable filtering for execution requests.
//!
//! Controls which environment variables are passed to sandboxed processes.

use crate::error::FilterError;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::violation::{ViolationAction, ViolationResult};

/// Configuration for environment variable filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvFilterConfig {
    /// Variables to always allow (whitelist).
    #[serde(default)]
    pub allow: Vec<String>,

    /// Variables to always block (denylist).
    #[serde(default)]
    pub deny: Vec<String>,

    /// Variables to mask in logs (but still pass to process).
    #[serde(default)]
    pub mask: Vec<String>,

    /// Whether to use allowlist mode (only allow listed vars).
    #[serde(default)]
    pub allowlist_mode: bool,

    /// Action when a denylisted variable is present.
    #[serde(default)]
    pub on_deny: ViolationAction,
}

impl Default for EnvFilterConfig {
    fn default() -> Self {
        Self {
            allow: Vec::new(),
            deny: Vec::new(),
            mask: Vec::new(),
            allowlist_mode: false,
            on_deny: ViolationAction::Warn, // Default: silently drop/warn
        }
    }
}

impl EnvFilterConfig {
    /// Create a new filter config with no restrictions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a variable to the allow list.
    pub fn allow(mut self, name: impl Into<String>) -> Self {
        self.allow.push(name.into());
        self
    }

    /// Add a variable to the deny list.
    pub fn deny(mut self, name: impl Into<String>) -> Self {
        self.deny.push(name.into());
        self
    }

    /// Add a variable to the mask list.
    pub fn mask(mut self, name: impl Into<String>) -> Self {
        self.mask.push(name.into());
        self
    }

    /// Enable allowlist mode.
    pub fn allowlist_mode(mut self, enabled: bool) -> Self {
        self.allowlist_mode = enabled;
        self
    }

    /// Set the action on deny.
    pub fn on_deny(mut self, action: ViolationAction) -> Self {
        self.on_deny = action;
        self
    }

    /// Create a config that blocks common secrets.
    pub fn with_common_secrets_blocked() -> Self {
        Self::new()
            .deny("AWS_SECRET_ACCESS_KEY")
            .deny("AWS_ACCESS_KEY_ID")
            .deny("GITHUB_TOKEN")
            .deny("GITLAB_TOKEN")
            .deny("SSH_PRIVATE_KEY")
            .deny("API_KEY")
            .deny("SECRET_KEY")
            .deny("PRIVATE_KEY")
            .deny("PASSWORD")
    }

    /// Create a config that only allows safe environment variables.
    /// Non-allowlisted variables are silently dropped (Warn on violation).
    pub fn safe_defaults() -> Self {
        Self::new()
            .allowlist_mode(true)
            .on_deny(ViolationAction::Warn) // Silently drop non-allowlisted vars
            .allow("PATH")
            .allow("LANG")
            .allow("LC_ALL")
            .allow("HOME")
            .allow("USER")
            .allow("TERM")
            .allow("TMPDIR")
            .allow("TEMP")
            .allow("TMP")
    }
}

/// Environment variable filter implementation.
#[derive(Debug)]
pub struct EnvFilter {
    config: EnvFilterConfig,
    allow_set: HashSet<String>,
    deny_set: HashSet<String>,
    mask_set: HashSet<String>,
}

impl EnvFilter {
    /// Create a new environment filter from config.
    pub fn new(config: EnvFilterConfig) -> Self {
        let allow_set: HashSet<String> = config.allow.iter().map(|s| s.to_uppercase()).collect();
        let deny_set: HashSet<String> = config.deny.iter().map(|s| s.to_uppercase()).collect();
        let mask_set: HashSet<String> = config.mask.iter().map(|s| s.to_uppercase()).collect();

        Self {
            config,
            allow_set,
            deny_set,
            mask_set,
        }
    }

    /// Create a filter with default config (no restrictions).
    pub fn empty() -> Self {
        Self::new(EnvFilterConfig::default())
    }

    /// Check if a variable name is allowed.
    pub fn is_allowed(&self, name: &str) -> bool {
        let name_upper = name.to_uppercase();

        // Check deny list first
        if self.deny_set.contains(&name_upper) {
            return false;
        }

        // In allowlist mode, only allow listed vars
        if self.config.allowlist_mode && !self.allow_set.contains(&name_upper) {
            return false;
        }

        true
    }

    /// Check if a variable should be masked in logs.
    pub fn should_mask(&self, name: &str) -> bool {
        self.mask_set.contains(&name.to_uppercase())
    }

    /// Check a single environment variable.
    pub fn check_env(&self, name: &str, _value: &str) -> ViolationResult {
        let name_upper = name.to_uppercase();

        // Check deny list
        if self.deny_set.contains(&name_upper) {
            return ViolationResult::violation(
                self.config.on_deny,
                format!("Environment variable '{}' is in deny list", name),
            );
        }

        // In allowlist mode, check if allowed
        if self.config.allowlist_mode && !self.allow_set.contains(&name_upper) {
            return ViolationResult::violation(
                self.config.on_deny,
                format!("Environment variable '{}' not in allow list", name),
            );
        }

        ViolationResult::ok()
    }

    /// Check all environment variables and return the first violation.
    pub fn check_envs<'a, I>(&self, envs: I) -> Option<(String, ViolationResult)>
    where
        I: IntoIterator<Item = (&'a str, &'a str)>,
    {
        for (name, value) in envs {
            let result = self.check_env(name, value);
            if result.violated {
                return Some((name.to_string(), result));
            }
        }
        None
    }

    /// Filter environment variables, keeping only allowed ones.
    /// Returns an error if a denylisted variable is present (when on_deny is Reject).
    pub fn filter_envs<'a, I>(&self, envs: I) -> Result<Vec<(String, String)>, FilterError>
    where
        I: IntoIterator<Item = &'a (String, String)>,
    {
        let mut filtered = Vec::new();
        for (name, value) in envs {
            let result = self.check_env(name, value);
            if result.should_reject() {
                return Err(FilterError::RejectedEnvironment {
                    name: name.clone(),
                    reason: result.reason,
                });
            }
            if result.violated {
                // Warn case - still skip the variable
                continue;
            }
            filtered.push((name.clone(), value.clone()));
        }
        Ok(filtered)
    }

    /// Get the list of allowed variable names (from config).
    pub fn allowed_vars(&self) -> &[String] {
        &self.config.allow
    }

    /// Get the list of denied variable names (from config).
    pub fn denied_vars(&self) -> &[String] {
        &self.config.deny
    }

    /// Get the list of masked variable names (from config).
    pub fn masked_vars(&self) -> &[String] {
        &self.config.mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_filter_config_default() {
        let config = EnvFilterConfig::default();
        assert!(config.allow.is_empty());
        assert!(config.deny.is_empty());
        assert!(config.mask.is_empty());
        assert!(!config.allowlist_mode);
    }

    #[test]
    fn env_filter_config_builder() {
        let config = EnvFilterConfig::new()
            .allow("PATH")
            .deny("SECRET")
            .mask("API_KEY")
            .allowlist_mode(true);

        assert_eq!(config.allow, vec!["PATH"]);
        assert_eq!(config.deny, vec!["SECRET"]);
        assert_eq!(config.mask, vec!["API_KEY"]);
        assert!(config.allowlist_mode);
    }

    #[test]
    fn env_filter_empty_allows_all() {
        let filter = EnvFilter::empty();
        assert!(filter.is_allowed("ANY_VAR"));
        assert!(filter.is_allowed("SECRET"));
    }

    #[test]
    fn env_filter_deny_blocks_specific() {
        let config = EnvFilterConfig::new()
            .deny("SECRET")
            .on_deny(ViolationAction::Reject);
        let filter = EnvFilter::new(config);

        assert!(!filter.is_allowed("SECRET"));
        assert!(!filter.is_allowed("secret")); // case insensitive
        assert!(filter.is_allowed("OTHER"));
    }

    #[test]
    fn env_filter_allowlist_mode() {
        let config = EnvFilterConfig::new()
            .allow("PATH")
            .allow("HOME")
            .allowlist_mode(true);
        let filter = EnvFilter::new(config);

        assert!(filter.is_allowed("PATH"));
        assert!(filter.is_allowed("path")); // case insensitive
        assert!(!filter.is_allowed("SECRET"));
    }

    #[test]
    fn env_filter_mask() {
        let config = EnvFilterConfig::new().mask("API_KEY");
        let filter = EnvFilter::new(config);

        assert!(filter.should_mask("API_KEY"));
        assert!(filter.should_mask("api_key")); // case insensitive
        assert!(!filter.should_mask("OTHER"));
    }

    #[test]
    fn env_filter_check_env_deny() {
        let config = EnvFilterConfig::new()
            .deny("SECRET")
            .on_deny(ViolationAction::Reject);
        let filter = EnvFilter::new(config);

        let result = filter.check_env("SECRET", "value");
        assert!(result.violated);
        assert!(result.should_reject());
    }

    #[test]
    fn env_filter_check_env_allowlist() {
        let config = EnvFilterConfig::new().allow("PATH").allowlist_mode(true);
        let filter = EnvFilter::new(config);

        let result = filter.check_env("SECRET", "value");
        assert!(result.violated);

        let result = filter.check_env("PATH", "/usr/bin");
        assert!(!result.violated);
    }

    #[test]
    fn env_filter_filter_envs() {
        let config = EnvFilterConfig::new()
            .deny("SECRET")
            .on_deny(ViolationAction::Reject);
        let filter = EnvFilter::new(config);

        let envs = [
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("SECRET".to_string(), "shhh".to_string()),
        ];

        let result = filter.filter_envs(envs.iter());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterError::RejectedEnvironment { .. }));
    }

    #[test]
    fn env_filter_filter_envs_passes_allowed() {
        let config = EnvFilterConfig::new().allow("PATH").allowlist_mode(true);
        let filter = EnvFilter::new(config);

        let envs = [
            ("PATH".to_string(), "/usr/bin".to_string()),
            ("SECRET".to_string(), "shhh".to_string()),
        ];

        let result = filter.filter_envs(envs.iter());
        assert!(result.is_ok());
        let filtered = result.unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "PATH");
    }

    #[test]
    fn env_filter_common_secrets_blocked() {
        let filter = EnvFilter::new(EnvFilterConfig::with_common_secrets_blocked());

        assert!(!filter.is_allowed("AWS_SECRET_ACCESS_KEY"));
        assert!(!filter.is_allowed("GITHUB_TOKEN"));
        assert!(!filter.is_allowed("PASSWORD"));
        assert!(filter.is_allowed("PATH"));
    }

    #[test]
    fn env_filter_safe_defaults() {
        let filter = EnvFilter::new(EnvFilterConfig::safe_defaults());

        assert!(filter.is_allowed("PATH"));
        assert!(filter.is_allowed("HOME"));
        assert!(!filter.is_allowed("SECRET"));
    }
}
