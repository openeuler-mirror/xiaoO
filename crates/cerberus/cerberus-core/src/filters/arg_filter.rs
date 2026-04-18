//! Argument filtering for execution requests.
//!
//! Provides pattern-based filtering and rejection of command arguments.

use crate::error::FilterError;
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::violation::{ViolationAction, ViolationResult};

/// Configuration for argument filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArgFilterConfig {
    /// Patterns to reject (regex patterns).
    #[serde(default)]
    pub reject_patterns: Vec<String>,

    /// Patterns to warn about (regex patterns).
    #[serde(default)]
    pub warn_patterns: Vec<String>,

    /// Maximum allowed argument length.
    #[serde(default = "default_max_arg_length")]
    pub max_arg_length: usize,

    /// Action when a rejection pattern matches.
    #[serde(default)]
    pub on_reject: ViolationAction,
}

fn default_max_arg_length() -> usize {
    8192 // 8KB default
}

impl Default for ArgFilterConfig {
    fn default() -> Self {
        Self {
            reject_patterns: Vec::new(),
            warn_patterns: Vec::new(),
            max_arg_length: default_max_arg_length(),
            on_reject: ViolationAction::Reject,
        }
    }
}

impl ArgFilterConfig {
    /// Create a new filter config with no restrictions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a rejection pattern.
    pub fn reject_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.reject_patterns.push(pattern.into());
        self
    }

    /// Add a warning pattern.
    pub fn warn_pattern(mut self, pattern: impl Into<String>) -> Self {
        self.warn_patterns.push(pattern.into());
        self
    }

    /// Set the maximum argument length.
    pub fn max_length(mut self, len: usize) -> Self {
        self.max_arg_length = len;
        self
    }

    /// Set the action on rejection.
    pub fn on_reject(mut self, action: ViolationAction) -> Self {
        self.on_reject = action;
        self
    }

    /// Create a config with common dangerous patterns.
    /// These patterns match individual arguments, not full command lines.
    pub fn with_common_dangerous_patterns() -> Self {
        Self::new()
            .reject_pattern(r"-rf$") // recursive force flag
            .reject_pattern(r"/$") // root path (context-dependent)
            .reject_pattern(r">\s*/dev/") // redirect to device
            .reject_pattern(r"mkfs\b") // format filesystem (note: no space)
            .reject_pattern(r"dd\s+if=") // dd if= (dangerous)
            .warn_pattern(r"--password") // password flags
            .warn_pattern(r"--secret") // secret flags
            .warn_pattern(r"-p") // common password flag shorthand
    }
}

/// Argument filter implementation.
#[derive(Debug)]
pub struct ArgFilter {
    config: ArgFilterConfig,
    reject_regexes: Vec<Regex>,
    warn_regexes: Vec<Regex>,
}

impl ArgFilter {
    /// Create a new argument filter from config.
    pub fn new(config: ArgFilterConfig) -> Result<Self, FilterError> {
        let reject_regexes = config
            .reject_patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                FilterError::OutputRedactionFailed(format!("Invalid reject pattern: {}", e))
            })?;

        let warn_regexes = config
            .warn_patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                FilterError::OutputRedactionFailed(format!("Invalid warn pattern: {}", e))
            })?;

        Ok(Self {
            config,
            reject_regexes,
            warn_regexes,
        })
    }

    /// Create a filter with default config (no restrictions).
    pub fn empty() -> Self {
        Self::new(ArgFilterConfig::default()).expect("default config should be valid")
    }

    /// Check a single argument against all rules.
    pub fn check_arg(&self, arg: &str) -> ViolationResult {
        // Check length
        if arg.len() > self.config.max_arg_length {
            return ViolationResult::violation(
                self.config.on_reject,
                format!(
                    "Argument exceeds max length {} (got {})",
                    self.config.max_arg_length,
                    arg.len()
                ),
            );
        }

        // Check rejection patterns
        for re in &self.reject_regexes {
            if re.is_match(arg) {
                return ViolationResult::violation(
                    self.config.on_reject,
                    format!("Argument matches reject pattern: {}", re),
                );
            }
        }

        // Check warning patterns
        for re in &self.warn_regexes {
            if re.is_match(arg) {
                return ViolationResult::violation(
                    ViolationAction::Warn,
                    format!("Argument matches warning pattern: {}", re),
                );
            }
        }

        ViolationResult::ok()
    }

    /// Check all arguments and return the first violation.
    pub fn check_args<'a, I>(&self, args: I) -> Option<(usize, ViolationResult)>
    where
        I: IntoIterator<Item = &'a str>,
    {
        for (idx, arg) in args.into_iter().enumerate() {
            let result = self.check_arg(arg);
            if result.violated {
                return Some((idx, result));
            }
        }
        None
    }

    /// Filter arguments, returning only those that pass.
    /// Note: This returns all args for warn, errors for reject.
    pub fn filter_args<'a, I>(&self, args: I) -> Result<Vec<&'a str>, FilterError>
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut filtered = Vec::new();
        for arg in args {
            let result = self.check_arg(arg);
            if result.should_reject() {
                return Err(FilterError::RejectedArgument {
                    value: arg.to_string(),
                    reason: result.reason,
                });
            }
            filtered.push(arg);
        }
        Ok(filtered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arg_filter_config_default() {
        let config = ArgFilterConfig::default();
        assert!(config.reject_patterns.is_empty());
        assert!(config.warn_patterns.is_empty());
        assert_eq!(config.max_arg_length, 8192);
    }

    #[test]
    fn arg_filter_config_builder() {
        let config = ArgFilterConfig::new()
            .reject_pattern(r"dangerous")
            .warn_pattern(r"warning")
            .max_length(100);

        assert_eq!(config.reject_patterns.len(), 1);
        assert_eq!(config.warn_patterns.len(), 1);
        assert_eq!(config.max_arg_length, 100);
    }

    #[test]
    fn arg_filter_empty_accepts_all() {
        let filter = ArgFilter::empty();
        let result = filter.check_arg("any argument");
        assert!(!result.violated);
    }

    #[test]
    fn arg_filter_rejects_dangerous() {
        let config = ArgFilterConfig::new()
            .reject_pattern(r"rm\s+-rf\s+/")
            .on_reject(ViolationAction::Reject);
        let filter = ArgFilter::new(config).unwrap();

        let result = filter.check_arg("rm -rf /");
        assert!(result.violated);
        assert!(result.should_reject());
    }

    #[test]
    fn arg_filter_warns_suspicious() {
        let config = ArgFilterConfig::new().warn_pattern(r"--password");
        let filter = ArgFilter::new(config).unwrap();

        let result = filter.check_arg("--password=secret");
        assert!(result.violated);
        assert_eq!(result.action, ViolationAction::Warn);
        assert!(!result.should_reject());
    }

    #[test]
    fn arg_filter_max_length() {
        let config = ArgFilterConfig::new().max_length(10);
        let filter = ArgFilter::new(config).unwrap();

        let result = filter.check_arg("this is way too long");
        assert!(result.violated);
        assert!(result.should_reject());
    }

    #[test]
    fn arg_filter_check_args_returns_first_violation() {
        let config = ArgFilterConfig::new()
            .reject_pattern(r"bad")
            .warn_pattern(r"warn");
        let filter = ArgFilter::new(config).unwrap();

        let args = ["good", "warning", "bad", "alsobad"];
        let result = filter.check_args(args.iter().copied());

        // First violation should be the warn at index 1
        let (idx, result) = result.unwrap();
        assert_eq!(idx, 1);
        assert_eq!(result.action, ViolationAction::Warn);
    }

    #[test]
    fn arg_filter_filter_args_rejects() {
        let config = ArgFilterConfig::new().reject_pattern(r"bad");
        let filter = ArgFilter::new(config).unwrap();

        let args = ["good", "bad", "good2"];
        let result = filter.filter_args(args.iter().copied());

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterError::RejectedArgument { .. }));
    }

    #[test]
    fn arg_filter_filter_args_passes_with_warn() {
        let config = ArgFilterConfig::new().warn_pattern(r"warn");
        let filter = ArgFilter::new(config).unwrap();

        let args = ["good", "warning", "good2"];
        let result = filter.filter_args(args.iter().copied());

        // Warn doesn't block
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 3);
    }

    #[test]
    fn arg_filter_common_dangerous_patterns() {
        let filter = ArgFilter::new(ArgFilterConfig::with_common_dangerous_patterns()).unwrap();

        // Should reject
        assert!(filter.check_arg("rm -rf /").violated);
        assert!(filter.check_arg("dd if=/dev/zero of=/dev/sda").violated);
        assert!(filter.check_arg("mkfs.ext4 /dev/sda1").violated);

        // Should warn
        let result = filter.check_arg("--password=secret");
        assert!(result.violated);
        assert_eq!(result.action, ViolationAction::Warn);
    }

    #[test]
    fn arg_filter_invalid_regex() {
        let config = ArgFilterConfig::new().reject_pattern(r"[invalid(");
        let result = ArgFilter::new(config);
        assert!(result.is_err());
    }
}
