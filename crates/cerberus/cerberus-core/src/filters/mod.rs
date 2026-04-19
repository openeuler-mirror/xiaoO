//! Filter module for request sanitization and output processing.
//!
//! This module provides filters that operate at two stages:
//! 1. **Request-time**: Argument and environment variable filtering
//! 2. **Output-time**: Output redaction, truncation, and deduplication
//!
//! Filters are helpers that prepare or post-process data - they do NOT
//! implement security enforcement (that's the sandbox module's job).
//!
//! # Example
//!
//! ```rust
//! use cerberus_core::filters::{ExecutionControl, ArgFilterConfig};
//! use cerberus_core::request::ExecRequest;
//!
//! let request = ExecRequest::new("ls").arg("-la");
//!
//! let mut control = ExecutionControl::builder(request)
//!     .arg_filter(ArgFilterConfig::new())
//!     .expect("valid config")
//!     .build();
//! let result = control.apply_arg_filters();
//! assert!(result.is_ok());
//! ```

mod arg_filter;
mod env_filter;
mod output_filter;
mod violation;

pub use arg_filter::{ArgFilter, ArgFilterConfig};
pub use env_filter::{EnvFilter, EnvFilterConfig};
pub use output_filter::{OutputFilter, OutputFilterConfig, OutputFilterResult, RedactPattern};
pub use violation::{ViolationAction, ViolationResult};

use crate::error::FilterError;
use crate::policy::Policy;
use crate::request::ExecRequest;
use crate::result::ExecResult;

/// Control structure for applying filters to execution requests.
///
/// This is the main entry point for the filter pipeline. It holds
/// the request and provides methods to apply each filter stage.
#[derive(Debug)]
pub struct ExecutionControl {
    /// The request being controlled.
    request: ExecRequest,
    /// Argument filter (if configured).
    arg_filter: Option<ArgFilter>,
    /// Environment filter (if configured).
    env_filter: Option<EnvFilter>,
    /// Output filter (if configured).
    output_filter: Option<OutputFilter>,
    /// Count of redactions applied.
    redactions_count: usize,
}

impl ExecutionControl {
    /// Create a new execution control with default filters.
    pub fn new(request: ExecRequest, _policy: &Policy) -> Self {
        Self {
            request,
            arg_filter: None,
            env_filter: None,
            output_filter: None,
            redactions_count: 0,
        }
    }

    /// Create a builder for configuring filters.
    pub fn builder(request: ExecRequest) -> ExecutionControlBuilder {
        ExecutionControlBuilder::new(request)
    }

    /// Get a reference to the request.
    pub fn request(&self) -> &ExecRequest {
        &self.request
    }

    /// Get the mutable request (for post-filter modifications).
    pub fn request_mut(&mut self) -> &mut ExecRequest {
        &mut self.request
    }

    /// Get the count of redactions applied.
    pub fn redactions_count(&self) -> usize {
        self.redactions_count
    }

    /// Apply argument filters.
    ///
    /// Returns an error if any argument is rejected.
    /// Updates the request with filtered arguments.
    pub fn apply_arg_filters(&mut self) -> Result<(), FilterError> {
        if let Some(ref filter) = self.arg_filter {
            let args: Vec<&str> = self.request.args.iter().map(|s| s.as_str()).collect();

            // Check for violations first
            if let Some((idx, result)) = filter.check_args(args.iter().copied()) {
                if result.should_reject() {
                    return Err(FilterError::RejectedArgument {
                        value: self.request.args[idx].clone(),
                        reason: result.reason,
                    });
                }
            }
        }
        Ok(())
    }

    /// Apply environment variable filters.
    ///
    /// Returns an error if a denylisted variable is present.
    /// Updates the request with filtered environment.
    pub fn apply_env_filters(&mut self) -> Result<(), FilterError> {
        if let Some(ref filter) = self.env_filter {
            let filtered = filter.filter_envs(self.request.env.iter())?;

            // Warn about masked variables (but don't block)
            for (name, _) in &self.request.env {
                if filter.should_mask(name) {
                    // Log warning or emit audit event
                }
            }

            self.request.env = filtered;
        }
        Ok(())
    }

    /// Apply output filters to an execution result.
    ///
    /// Returns the modified result with redactions applied.
    pub fn apply_output_filters(&self, mut result: ExecResult) -> ExecResult {
        if let Some(ref filter) = self.output_filter {
            let stdout_result = filter.filter(&result.stdout);
            result.stdout = stdout_result.output;

            let stderr_result = filter.filter(&result.stderr);
            result.stderr = stderr_result.output;

            let total_redactions = stdout_result.redactions + stderr_result.redactions;
            result.metadata.redactions_applied = total_redactions;

            // Note: termination would need to be handled by caller
            // if stdout_result.should_terminate || stderr_result.should_terminate {
            //     // Would need to signal termination
            // }
        }
        result
    }

    /// Apply all filters and return the modified request.
    pub fn apply_all(mut self) -> Result<FilteredRequest, FilterError> {
        self.apply_arg_filters()?;
        self.apply_env_filters()?;

        Ok(FilteredRequest {
            request: self.request,
            redactions_count: self.redactions_count,
            output_filter: self.output_filter,
        })
    }
}

/// Result of applying all filters to a request.
#[derive(Debug)]
pub struct FilteredRequest {
    /// The filtered request.
    pub request: ExecRequest,
    /// Number of redactions applied during filtering.
    pub redactions_count: usize,
    /// Output filter to apply after execution.
    pub output_filter: Option<OutputFilter>,
}

impl FilteredRequest {
    /// Apply output filters to an execution result.
    pub fn filter_output(&self, result: ExecResult) -> ExecResult {
        if let Some(ref filter) = self.output_filter {
            let mut result = result;

            let stdout_result = filter.filter(&result.stdout);
            result.stdout = stdout_result.output;

            let stderr_result = filter.filter(&result.stderr);
            result.stderr = stderr_result.output;

            result.metadata.redactions_applied =
                stdout_result.redactions + stderr_result.redactions;

            result
        } else {
            result
        }
    }
}

/// Builder for configuring execution control filters.
#[derive(Debug)]
pub struct ExecutionControlBuilder {
    request: ExecRequest,
    arg_filter: Option<ArgFilter>,
    env_filter: Option<EnvFilter>,
    output_filter: Option<OutputFilter>,
}

impl ExecutionControlBuilder {
    /// Create a new builder with the given request.
    pub fn new(request: ExecRequest) -> Self {
        Self {
            request,
            arg_filter: None,
            env_filter: None,
            output_filter: None,
        }
    }

    /// Set the argument filter.
    pub fn arg_filter(mut self, config: ArgFilterConfig) -> Result<Self, FilterError> {
        self.arg_filter = Some(ArgFilter::new(config)?);
        Ok(self)
    }

    /// Set the environment filter.
    pub fn env_filter(mut self, config: EnvFilterConfig) -> Self {
        self.env_filter = Some(EnvFilter::new(config));
        self
    }

    /// Set the output filter.
    pub fn output_filter(mut self, config: OutputFilterConfig) -> Result<Self, FilterError> {
        self.output_filter = Some(OutputFilter::new(config)?);
        Ok(self)
    }

    /// Build the execution control.
    pub fn build(self) -> ExecutionControl {
        ExecutionControl {
            request: self.request,
            arg_filter: self.arg_filter,
            env_filter: self.env_filter,
            output_filter: self.output_filter,
            redactions_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_control_new() {
        let request = ExecRequest::new("ls").arg("-la");
        let policy = Policy::minimal();

        let control = ExecutionControl::new(request, &policy);
        assert_eq!(control.request().program, "ls");
        assert_eq!(control.request().args, vec!["-la"]);
    }

    #[test]
    fn execution_control_builder() {
        let request = ExecRequest::new("echo").arg("hello");

        let control = ExecutionControl::builder(request)
            .arg_filter(ArgFilterConfig::new())
            .expect("valid config")
            .env_filter(EnvFilterConfig::new())
            .output_filter(OutputFilterConfig::new())
            .expect("valid config")
            .build();

        assert!(control.arg_filter.is_some());
        assert!(control.env_filter.is_some());
        assert!(control.output_filter.is_some());
    }

    #[test]
    fn execution_control_apply_arg_filters_passes() {
        let request = ExecRequest::new("ls").arg("-la");

        let mut control = ExecutionControl::builder(request)
            .arg_filter(ArgFilterConfig::new())
            .expect("valid config")
            .build();

        let result = control.apply_arg_filters();
        assert!(result.is_ok());
    }

    #[test]
    fn execution_control_apply_arg_filters_rejects() {
        let request = ExecRequest::new("rm").args(["-rf", "/"]);

        let mut control = ExecutionControl::builder(request)
            .arg_filter(ArgFilterConfig::with_common_dangerous_patterns())
            .expect("valid config")
            .build();

        let result = control.apply_arg_filters();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FilterError::RejectedArgument { .. }));
    }

    #[test]
    fn execution_control_apply_env_filters_deny() {
        let request = ExecRequest::new("echo").env("SECRET", "value");

        let mut control = ExecutionControl::builder(request)
            .env_filter(
                EnvFilterConfig::new()
                    .deny("SECRET")
                    .on_deny(ViolationAction::Reject),
            )
            .build();

        let result = control.apply_env_filters();
        assert!(result.is_err());
    }

    #[test]
    fn execution_control_apply_env_filters_allowlist() {
        let request = ExecRequest::new("echo")
            .env("PATH", "/usr/bin")
            .env("SECRET", "value");

        let mut control = ExecutionControl::builder(request)
            .env_filter(EnvFilterConfig::safe_defaults())
            .build();

        let result = control.apply_env_filters();
        assert!(result.is_ok());

        // Only PATH should remain
        assert_eq!(control.request().env.len(), 1);
        assert_eq!(control.request().env[0].0, "PATH");
    }

    #[test]
    fn execution_control_apply_output_filters() {
        let request = ExecRequest::new("echo");

        let control = ExecutionControl::builder(request)
            .output_filter(OutputFilterConfig::new().redact(r"secret"))
            .expect("valid config")
            .build();

        let result = ExecResult::success().stdout(b"the secret is hidden".to_vec());

        let filtered = control.apply_output_filters(result);

        assert_eq!(filtered.stdout, b"the [REDACTED] is hidden".to_vec());
        assert_eq!(filtered.metadata.redactions_applied, 1);
    }

    #[test]
    fn execution_control_apply_all() {
        let request = ExecRequest::new("echo")
            .arg("hello")
            .env("PATH", "/usr/bin");

        let filtered = ExecutionControl::builder(request)
            .arg_filter(ArgFilterConfig::new())
            .expect("valid config")
            .env_filter(EnvFilterConfig::new())
            .output_filter(OutputFilterConfig::new())
            .expect("valid config")
            .build()
            .apply_all()
            .expect("filters should pass");

        assert_eq!(filtered.request.program, "echo");
        assert_eq!(filtered.request.args, vec!["hello"]);
    }

    #[test]
    fn filtered_request_filter_output() {
        let request = ExecRequest::new("echo");

        let filtered = ExecutionControl::builder(request)
            .output_filter(OutputFilterConfig::new().max_bytes(10))
            .expect("valid config")
            .build()
            .apply_all()
            .expect("should pass");

        let result = ExecResult::success().stdout(b"this is a long string".to_vec());

        let filtered_result = filtered.filter_output(result);
        assert!(filtered_result.stdout.len() <= 10);
    }
}
