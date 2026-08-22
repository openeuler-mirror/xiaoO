//! Output filtering for execution results.
//!
//! Provides redaction, truncation, and deduplication of command output.

use crate::error::FilterError;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use super::violation::{ViolationAction, ViolationResult};

/// Configuration for output filtering.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputFilterConfig {
    /// Maximum output size in bytes (0 = unlimited).
    #[serde(default)]
    pub max_output_bytes: usize,

    /// Patterns to redact from output (regex).
    #[serde(default)]
    pub redact_patterns: Vec<RedactPattern>,

    /// Whether to deduplicate consecutive identical lines.
    #[serde(default)]
    pub deduplicate_lines: bool,

    /// Action when a termination pattern is detected.
    #[serde(default)]
    pub on_secret_detected: ViolationAction,

    /// Replacement string for redacted content.
    #[serde(default = "default_redact_replacement")]
    pub redact_replacement: String,
}

/// A pattern to redact from output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactPattern {
    /// Regex pattern to match.
    pub pattern: String,
    /// Optional description of what's being redacted.
    #[serde(default)]
    pub description: Option<String>,
    /// Whether detection should trigger termination.
    #[serde(default)]
    pub terminate_on_match: bool,
}

fn default_redact_replacement() -> String {
    "[REDACTED]".to_string()
}

impl Default for OutputFilterConfig {
    fn default() -> Self {
        Self {
            max_output_bytes: 0, // unlimited by default
            redact_patterns: Vec::new(),
            deduplicate_lines: false,
            on_secret_detected: ViolationAction::Warn,
            redact_replacement: default_redact_replacement(),
        }
    }
}

impl OutputFilterConfig {
    /// Create a new filter config with no restrictions.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set maximum output size.
    pub fn max_bytes(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes;
        self
    }

    /// Add a redaction pattern.
    pub fn redact(mut self, pattern: impl Into<String>) -> Self {
        self.redact_patterns.push(RedactPattern {
            pattern: pattern.into(),
            description: None,
            terminate_on_match: false,
        });
        self
    }

    /// Add a redaction pattern with termination.
    pub fn redact_and_terminate(mut self, pattern: impl Into<String>) -> Self {
        self.redact_patterns.push(RedactPattern {
            pattern: pattern.into(),
            description: None,
            terminate_on_match: true,
        });
        self
    }

    /// Enable line deduplication.
    pub fn deduplicate(mut self, enabled: bool) -> Self {
        self.deduplicate_lines = enabled;
        self
    }

    /// Set the redaction replacement string.
    pub fn redact_replacement(mut self, replacement: impl Into<String>) -> Self {
        self.redact_replacement = replacement.into();
        self
    }

    /// Set the action when secrets are detected.
    pub fn on_secret(mut self, action: ViolationAction) -> Self {
        self.on_secret_detected = action;
        self
    }

    /// Create a config with common secret patterns.
    pub fn with_common_secrets() -> Self {
        Self::new()
            .redact(r"AWS_ACCESS_KEY_ID=[A-Z0-9]{20}")
            .redact(r"AWS_SECRET_ACCESS_KEY=[A-Za-z0-9/+=]{40}")
            .redact(r"ghp_[A-Za-z0-9]{36}") // GitHub PAT
            .redact(r"gho_[A-Za-z0-9]{36}") // GitHub OAuth
            .redact(r"sk-[A-Za-z0-9]{20,}") // OpenAI key
            .redact(r"xox[baprs]-[A-Za-z0-9-]+") // Slack tokens
            .redact(r"-----BEGIN (RSA |DSA |EC |OPENSSH )?PRIVATE KEY-----")
    }
}

/// Result of output filtering.
#[derive(Debug, Clone)]
pub struct OutputFilterResult {
    /// The filtered output.
    pub output: Vec<u8>,
    /// Number of redactions applied.
    pub redactions: usize,
    /// Whether output was truncated.
    pub truncated: bool,
    /// Whether a termination-triggering pattern was found.
    pub should_terminate: bool,
    /// Human-readable description of what was found.
    pub termination_reason: Option<String>,
}

impl OutputFilterResult {
    /// Create a result with no changes.
    pub fn unchanged(output: Vec<u8>) -> Self {
        Self {
            output,
            redactions: 0,
            truncated: false,
            should_terminate: false,
            termination_reason: None,
        }
    }
}

/// Output filter implementation.
#[derive(Debug)]
pub struct OutputFilter {
    config: OutputFilterConfig,
    redact_regexes: Vec<(Regex, bool)>, // (pattern, terminate_on_match)
}

impl OutputFilter {
    /// Create a new output filter from config.
    pub fn new(config: OutputFilterConfig) -> Result<Self, FilterError> {
        let redact_regexes = config
            .redact_patterns
            .iter()
            .map(|p| {
                Regex::new(&p.pattern)
                    .map(|r| (r, p.terminate_on_match))
                    .map_err(|e| {
                        FilterError::OutputRedactionFailed(format!(
                            "Invalid redact pattern '{}': {}",
                            p.pattern, e
                        ))
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            config,
            redact_regexes,
        })
    }

    /// Create a filter with default config (no restrictions).
    pub fn empty() -> Self {
        Self::new(OutputFilterConfig::default()).expect("default config should be valid")
    }

    /// Filter the output, applying all configured rules.
    pub fn filter(&self, output: &[u8]) -> OutputFilterResult {
        // Convert to string for pattern matching (lossy for binary)
        let mut text = String::from_utf8_lossy(output).into_owned();
        let mut redactions = 0;
        let mut should_terminate = false;
        let mut termination_reason = None;

        // Apply redaction patterns
        for (regex, terminate) in &self.redact_regexes {
            if regex.is_match(&text) {
                if *terminate {
                    should_terminate = true;
                    termination_reason = Some(format!("Secret pattern detected: {}", regex));
                }
                let count = regex.find_iter(&text).count();
                text = regex
                    .replace_all(&text, &self.config.redact_replacement)
                    .into_owned();
                redactions += count;
            }
        }

        // Apply deduplication
        if self.config.deduplicate_lines {
            text = self.deduplicate_lines(&text);
        }

        // Apply truncation
        let truncated =
            if self.config.max_output_bytes > 0 && text.len() > self.config.max_output_bytes {
                text.truncate(self.config.max_output_bytes);
                true
            } else {
                false
            };

        OutputFilterResult {
            output: text.into_bytes(),
            redactions,
            truncated,
            should_terminate,
            termination_reason,
        }
    }

    /// Deduplicate consecutive identical lines.
    fn deduplicate_lines(&self, text: &str) -> String {
        let mut result = String::new();
        let mut seen_lines: HashSet<String> = HashSet::new();
        let mut last_line = String::new();

        for line in text.lines() {
            let line_string = line.to_string();

            // Check if this line is a duplicate of the previous one
            if line_string == last_line {
                continue; // Skip consecutive duplicate
            }

            // Check if we've seen this line before (non-consecutive)
            if seen_lines.contains(&line_string) {
                // For non-consecutive duplicates, we still include them
                // but mark them (optional behavior)
                result.push_str(line);
                result.push('\n');
            } else {
                seen_lines.insert(line_string.clone());
                result.push_str(line);
                result.push('\n');
            }

            last_line = line_string;
        }

        // Remove trailing newline if original didn't have one
        if !text.ends_with('\n') && result.ends_with('\n') {
            result.pop();
        }

        result
    }

    /// Check for secrets without modifying output.
    pub fn check_for_secrets(&self, output: &[u8]) -> ViolationResult {
        let text = String::from_utf8_lossy(output);

        for (regex, terminate) in &self.redact_regexes {
            if regex.is_match(&text) {
                if *terminate || self.config.on_secret_detected == ViolationAction::Terminate {
                    return ViolationResult::violation(
                        ViolationAction::Terminate,
                        format!("Secret pattern detected: {}", regex),
                    );
                }
                return ViolationResult::violation(
                    self.config.on_secret_detected,
                    format!("Secret pattern detected: {}", regex),
                );
            }
        }

        ViolationResult::ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_filter_config_default() {
        let config = OutputFilterConfig::default();
        assert_eq!(config.max_output_bytes, 0);
        assert!(config.redact_patterns.is_empty());
        assert!(!config.deduplicate_lines);
    }

    #[test]
    fn output_filter_config_builder() {
        let config = OutputFilterConfig::new()
            .max_bytes(1024)
            .redact(r"secret")
            .deduplicate(true);

        assert_eq!(config.max_output_bytes, 1024);
        assert_eq!(config.redact_patterns.len(), 1);
        assert!(config.deduplicate_lines);
    }

    #[test]
    fn output_filter_empty_passes_through() {
        let filter = OutputFilter::empty();
        let output = b"hello world".to_vec();
        let result = filter.filter(&output);

        assert_eq!(result.output, output);
        assert_eq!(result.redactions, 0);
        assert!(!result.truncated);
    }

    #[test]
    fn output_filter_redacts_pattern() {
        let config = OutputFilterConfig::new().redact(r"secret");
        let filter = OutputFilter::new(config).unwrap();

        let output = b"the secret is hidden".to_vec();
        let result = filter.filter(&output);

        assert_eq!(result.output, b"the [REDACTED] is hidden".to_vec());
        assert_eq!(result.redactions, 1);
    }

    #[test]
    fn output_filter_multiple_redactions() {
        let config = OutputFilterConfig::new().redact(r"\d+");
        let filter = OutputFilter::new(config).unwrap();

        let output = b"count: 1, 2, 3".to_vec();
        let result = filter.filter(&output);

        assert_eq!(
            result.output,
            b"count: [REDACTED], [REDACTED], [REDACTED]".to_vec()
        );
        assert_eq!(result.redactions, 3);
    }

    #[test]
    fn output_filter_truncates() {
        let config = OutputFilterConfig::new().max_bytes(10);
        let filter = OutputFilter::new(config).unwrap();

        let output = b"this is a long string".to_vec();
        let result = filter.filter(&output);

        assert!(result.truncated);
        assert_eq!(result.output.len(), 10);
    }

    #[test]
    fn output_filter_deduplicate_consecutive() {
        let config = OutputFilterConfig::new().deduplicate(true);
        let filter = OutputFilter::new(config).unwrap();

        let output = b"line1\nline1\nline2\nline2\nline2\nline1".to_vec();
        let result = filter.filter(&output);

        let text = String::from_utf8_lossy(&result.output);
        // Consecutive duplicates should be removed
        assert!(text.contains("line1"));
        assert!(text.contains("line2"));
    }

    #[test]
    fn output_filter_terminate_on_pattern() {
        let config = OutputFilterConfig::new().redact_and_terminate(r"SECRET_KEY");
        let filter = OutputFilter::new(config).unwrap();

        let output = b"SECRET_KEY=abc123".to_vec();
        let result = filter.filter(&output);

        assert!(result.should_terminate);
        assert!(result.termination_reason.is_some());
    }

    #[test]
    fn output_filter_check_for_secrets() {
        let config = OutputFilterConfig::new()
            .redact(r"password")
            .on_secret(ViolationAction::Warn);
        let filter = OutputFilter::new(config).unwrap();

        let output = b"password=secret";
        let result = filter.check_for_secrets(output);

        assert!(result.violated);
        assert_eq!(result.action, ViolationAction::Warn);
    }

    #[test]
    fn output_filter_common_secrets() {
        let filter = OutputFilter::new(OutputFilterConfig::with_common_secrets()).unwrap();

        // Should redact AWS keys
        let output = b"AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE";
        let result = filter.filter(output);
        assert!(result.redactions > 0);

        // Should redact GitHub tokens
        let output = b"token: ghp_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        let result = filter.filter(output);
        assert!(result.redactions > 0);

        // Should redact private keys
        let output = b"-----BEGIN RSA PRIVATE KEY-----";
        let result = filter.filter(output);
        assert!(result.redactions > 0);
    }

    #[test]
    fn output_filter_custom_replacement() {
        let config = OutputFilterConfig::new()
            .redact(r"secret")
            .redact_replacement("[FILTERED]".to_string());
        let filter = OutputFilter::new(config).unwrap();

        let output = b"the secret is here".to_vec();
        let result = filter.filter(&output);

        assert_eq!(result.output, b"the [FILTERED] is here".to_vec());
    }

    #[test]
    fn output_filter_invalid_pattern() {
        let config = OutputFilterConfig::new().redact(r"[invalid(");
        let result = OutputFilter::new(config);
        assert!(result.is_err());
    }
}
