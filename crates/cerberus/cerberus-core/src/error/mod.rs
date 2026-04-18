//! Layered error model for Cerberus.
//!
//! This module provides a structured error hierarchy that allows SDK callers
//! to precisely match error types and CLI to render user-friendly messages.

use std::path::PathBuf;
use std::time::Duration;
use thiserror::Error;

/// Top-level error type for Cerberus operations.
///
/// This is the primary error type returned by `execute()` and other
/// public APIs. SDK callers can match on variants to handle specific
/// error categories, while CLI can render appropriate user messages.
#[derive(Debug, Error)]
pub enum CerberusError {
    /// Request validation failed before execution.
    #[error("Request error: {0}")]
    Request(#[from] RequestError),

    /// Policy configuration or validation failed.
    #[error("Policy error: {0}")]
    Policy(#[from] PolicyError),

    /// Execution-time filtering rejected or modified the request.
    #[error("Filter error: {0}")]
    Filter(#[from] FilterError),

    /// Sandbox isolation setup failed.
    #[error("Sandbox setup error: {0}")]
    SandboxSetup(#[from] SandboxSetupError),

    /// Process execution failed or produced unexpected result.
    #[error("Execution error: {0}")]
    Execution(#[from] ExecutionError),

    /// Audit logging or event emission failed.
    #[error("Audit error: {0}")]
    Audit(#[from] AuditError),
}

/// Errors related to execution request validation.
///
/// These errors occur before any execution attempt, indicating
/// that the request itself is malformed or invalid.
#[derive(Debug, Error)]
pub enum RequestError {
    /// Program path is empty or invalid.
    #[error("Empty or invalid program path")]
    EmptyProgram,

    /// Working directory does not exist or is not accessible.
    #[error("Invalid working directory: {0}")]
    InvalidWorkingDirectory(PathBuf),

    /// Command argument is invalid.
    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    /// Environment variable specification is invalid.
    #[error("Invalid environment: {0}")]
    InvalidEnvironment(String),
}

/// Errors related to policy configuration and validation.
///
/// These errors indicate problems with the security policy
/// definition, not with policy enforcement during execution.
#[derive(Debug, Error)]
pub enum PolicyError {
    /// Filesystem rule is invalid or conflicts.
    #[error("Invalid filesystem rule: {0}")]
    InvalidFsRule(String),

    /// Network rule is invalid or conflicts.
    #[error("Invalid network rule: {0}")]
    InvalidNetworkRule(String),

    /// Resource limit specification is invalid.
    #[error("Invalid resource limit: {0}")]
    InvalidResourceLimit(String),

    /// Policy rules conflict with each other.
    #[error("Policy conflict: {0}")]
    Conflict(String),

    /// Failed to load policy profile.
    #[error("Failed to load policy profile: {0}")]
    ProfileLoadFailed(String),
}

/// Errors from execution-time filtering and control.
///
/// These errors occur when filters reject arguments, environment
/// variables, or when violations trigger termination.
#[derive(Debug, Error)]
pub enum FilterError {
    /// Argument was rejected by filter rules.
    #[error("Rejected argument '{value}': {reason}")]
    RejectedArgument {
        /// The argument value that was rejected.
        value: String,
        /// Human-readable reason for rejection.
        reason: String,
    },

    /// Environment variable was rejected by filter rules.
    #[error("Rejected environment variable '{name}': {reason}")]
    RejectedEnvironment {
        /// The environment variable name that was rejected.
        name: String,
        /// Human-readable reason for rejection.
        reason: String,
    },

    /// Output redaction encountered an error.
    #[error("Output redaction failed: {0}")]
    OutputRedactionFailed(String),

    /// Violation triggered the configured action.
    #[error("Violation triggered {action}: {reason}")]
    ViolationTriggered {
        /// The action that was triggered (e.g., "terminate", "reject").
        action: String,
        /// Human-readable reason for the violation.
        reason: String,
    },
}

/// Errors during sandbox isolation setup.
///
/// These errors occur when setting up OS-level isolation
/// mechanisms like Landlock, seccomp, or namespaces.
#[derive(Debug, Error)]
pub enum SandboxSetupError {
    /// Platform does not support sandboxing.
    #[error("Unsupported platform: sandboxing requires Linux")]
    UnsupportedPlatform,

    /// Namespace setup failed.
    #[error("Namespace setup failed: {0}")]
    NamespaceSetupFailed(String),

    /// Landlock LSM setup failed.
    #[error("Landlock setup failed: {0}")]
    LandlockSetupFailed(String),

    /// Seccomp filter setup failed.
    #[error("Seccomp setup failed: {0}")]
    SeccompSetupFailed(String),

    /// Mount isolation setup failed.
    #[error("Mount isolation failed: {0}")]
    MountIsolationFailed(String),

    /// eBPF program setup failed.
    #[error("eBPF setup failed: {0}")]
    EbpfSetupFailed(String),

    /// Sandbox capabilities insufficient for policy enforcement.
    #[error("Sandbox capability error: {feature} unavailable - {reason}")]
    CapabilityError {
        /// The feature that is unavailable.
        feature: String,
        /// Human-readable reason for the capability error.
        reason: String,
    },
}

/// Errors during process execution.
///
/// These errors occur during the actual execution of the
/// child process, after sandbox setup has completed.
#[derive(Debug, Error)]
pub enum ExecutionError {
    /// Failed to spawn the child process.
    #[error("Failed to spawn process: {0}")]
    SpawnFailed(String),

    /// Failed to wait for process completion.
    #[error("Failed to wait for process: {0}")]
    WaitFailed(String),

    /// Process execution exceeded timeout.
    #[error("Execution timed out after {duration:?}")]
    TimedOut {
        /// The timeout duration that was exceeded.
        duration: Duration,
    },

    /// Process was killed by signal.
    #[error("Process killed by signal")]
    Killed {
        /// Signal number if available.
        signal: Option<i32>,
    },

    /// Process exited with non-zero status.
    #[error("Process exited with code {exit_code}: {stderr}")]
    ExitNonZero {
        /// The exit code returned by the process.
        exit_code: i32,
        /// Captured stderr output.
        stderr: String,
    },
}

/// Errors related to audit logging and event emission.
///
/// These errors occur when audit sinks fail to record events
/// or when event encoding fails.
#[derive(Debug, Error)]
pub enum AuditError {
    /// Audit sink failed to record event.
    #[error("Audit sink failed: {0}")]
    SinkFailed(String),

    /// Event could not be encoded for audit output.
    #[error("Audit event encoding failed: {0}")]
    EventEncodingFailed(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================
    // CerberusError Display and Conversion Tests
    // ========================================

    #[test]
    fn cerberus_error_from_request_error() {
        let err = RequestError::EmptyProgram;
        let cerr: CerberusError = err.into();
        assert!(cerr.to_string().contains("Request error"));
        assert!(cerr.to_string().contains("Empty or invalid program path"));
    }

    #[test]
    fn cerberus_error_from_policy_error() {
        let err = PolicyError::Conflict("fs vs network".into());
        let cerr: CerberusError = err.into();
        assert!(cerr.to_string().contains("Policy error"));
        assert!(cerr.to_string().contains("fs vs network"));
    }

    #[test]
    fn cerberus_error_from_filter_error() {
        let err = FilterError::RejectedArgument {
            value: "--dangerous".into(),
            reason: "pattern match".into(),
        };
        let cerr: CerberusError = err.into();
        assert!(cerr.to_string().contains("Filter error"));
        assert!(cerr.to_string().contains("--dangerous"));
    }

    #[test]
    fn cerberus_error_from_sandbox_setup_error() {
        let err = SandboxSetupError::UnsupportedPlatform;
        let cerr: CerberusError = err.into();
        assert!(cerr.to_string().contains("Sandbox setup error"));
        assert!(cerr.to_string().contains("Unsupported platform"));
    }

    #[test]
    fn cerberus_error_from_execution_error() {
        let err = ExecutionError::TimedOut {
            duration: Duration::from_secs(30),
        };
        let cerr: CerberusError = err.into();
        assert!(cerr.to_string().contains("Execution error"));
        assert!(cerr.to_string().contains("timed out"));
    }

    #[test]
    fn cerberus_error_from_audit_error() {
        let err = AuditError::SinkFailed("disk full".into());
        let cerr: CerberusError = err.into();
        assert!(cerr.to_string().contains("Audit error"));
        assert!(cerr.to_string().contains("disk full"));
    }

    // ========================================
    // RequestError Tests
    // ========================================

    #[test]
    fn request_error_empty_program() {
        let err = RequestError::EmptyProgram;
        assert!(err.to_string().contains("Empty or invalid program path"));
    }

    #[test]
    fn request_error_invalid_working_directory() {
        let err = RequestError::InvalidWorkingDirectory(PathBuf::from("/nonexistent"));
        assert!(err.to_string().contains("/nonexistent"));
    }

    #[test]
    fn request_error_invalid_argument() {
        let err = RequestError::InvalidArgument("contains null byte".into());
        assert!(err.to_string().contains("contains null byte"));
    }

    #[test]
    fn request_error_invalid_environment() {
        let err = RequestError::InvalidEnvironment("key without value".into());
        assert!(err.to_string().contains("key without value"));
    }

    // ========================================
    // PolicyError Tests
    // ========================================

    #[test]
    fn policy_error_invalid_fs_rule() {
        let err = PolicyError::InvalidFsRule("path does not exist".into());
        assert!(err.to_string().contains("Invalid filesystem rule"));
        assert!(err.to_string().contains("path does not exist"));
    }

    #[test]
    fn policy_error_invalid_network_rule() {
        let err = PolicyError::InvalidNetworkRule("malformed CIDR".into());
        assert!(err.to_string().contains("Invalid network rule"));
    }

    #[test]
    fn policy_error_invalid_resource_limit() {
        let err = PolicyError::InvalidResourceLimit("negative memory".into());
        assert!(err.to_string().contains("Invalid resource limit"));
    }

    #[test]
    fn policy_error_conflict() {
        let err = PolicyError::Conflict("read-only vs write access".into());
        assert!(err.to_string().contains("Policy conflict"));
    }

    #[test]
    fn policy_error_profile_load_failed() {
        let err = PolicyError::ProfileLoadFailed("file not found: policy.toml".into());
        assert!(err.to_string().contains("Failed to load policy profile"));
    }

    // ========================================
    // FilterError Tests
    // ========================================

    #[test]
    fn filter_error_rejected_argument() {
        let err = FilterError::RejectedArgument {
            value: "rm -rf /".into(),
            reason: "matches dangerous pattern".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("rm -rf /"));
        assert!(msg.contains("matches dangerous pattern"));
    }

    #[test]
    fn filter_error_rejected_environment() {
        let err = FilterError::RejectedEnvironment {
            name: "AWS_SECRET_KEY".into(),
            reason: "secret in deny list".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("AWS_SECRET_KEY"));
        assert!(msg.contains("secret in deny list"));
    }

    #[test]
    fn filter_error_output_redaction_failed() {
        let err = FilterError::OutputRedactionFailed("regex error".into());
        assert!(err.to_string().contains("Output redaction failed"));
        assert!(err.to_string().contains("regex error"));
    }

    #[test]
    fn filter_error_violation_triggered() {
        let err = FilterError::ViolationTriggered {
            action: "terminate".into(),
            reason: "secret detected in output".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("terminate"));
        assert!(msg.contains("secret detected in output"));
    }

    // ========================================
    // SandboxSetupError Tests
    // ========================================

    #[test]
    fn sandbox_setup_error_unsupported_platform() {
        let err = SandboxSetupError::UnsupportedPlatform;
        assert!(err.to_string().contains("Unsupported platform"));
        assert!(err.to_string().contains("Linux"));
    }

    #[test]
    fn sandbox_setup_error_namespace_failed() {
        let err = SandboxSetupError::NamespaceSetupFailed("EPERM".into());
        assert!(err.to_string().contains("Namespace setup failed"));
        assert!(err.to_string().contains("EPERM"));
    }

    #[test]
    fn sandbox_setup_error_landlock_failed() {
        let err = SandboxSetupError::LandlockSetupFailed("kernel too old".into());
        assert!(err.to_string().contains("Landlock setup failed"));
    }

    #[test]
    fn sandbox_setup_error_seccomp_failed() {
        let err = SandboxSetupError::SeccompSetupFailed("BPF load error".into());
        assert!(err.to_string().contains("Seccomp setup failed"));
    }

    #[test]
    fn sandbox_setup_error_mount_failed() {
        let err = SandboxSetupError::MountIsolationFailed("bind mount refused".into());
        assert!(err.to_string().contains("Mount isolation failed"));
    }

    #[test]
    fn sandbox_setup_error_ebpf_failed() {
        let err = SandboxSetupError::EbpfSetupFailed("map creation failed".into());
        assert!(err.to_string().contains("eBPF setup failed"));
    }

    #[test]
    fn sandbox_setup_error_capability_error() {
        let err = SandboxSetupError::CapabilityError {
            feature: "landlock".into(),
            reason: "kernel too old".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Sandbox capability error"));
        assert!(msg.contains("landlock"));
        assert!(msg.contains("kernel too old"));
    }

    // ========================================
    // ExecutionError Tests
    // ========================================

    #[test]
    fn execution_error_spawn_failed() {
        let err = ExecutionError::SpawnFailed("execve: permission denied".into());
        assert!(err.to_string().contains("Failed to spawn process"));
        assert!(err.to_string().contains("permission denied"));
    }

    #[test]
    fn execution_error_wait_failed() {
        let err = ExecutionError::WaitFailed("child already reaped".into());
        assert!(err.to_string().contains("Failed to wait for process"));
    }

    #[test]
    fn execution_error_timed_out() {
        let err = ExecutionError::TimedOut {
            duration: Duration::from_secs(60),
        };
        let msg = err.to_string();
        assert!(msg.contains("timed out"));
        assert!(msg.contains("60s"));
    }

    #[test]
    fn execution_error_killed_with_signal() {
        let err = ExecutionError::Killed { signal: Some(9) };
        assert!(err.to_string().contains("killed"));
    }

    #[test]
    fn execution_error_killed_without_signal() {
        let err = ExecutionError::Killed { signal: None };
        assert!(err.to_string().contains("killed"));
    }

    #[test]
    fn execution_error_exit_non_zero() {
        let err = ExecutionError::ExitNonZero {
            exit_code: 127,
            stderr: "command not found".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("127"));
        assert!(msg.contains("command not found"));
    }

    // ========================================
    // AuditError Tests
    // ========================================

    #[test]
    fn audit_error_sink_failed() {
        let err = AuditError::SinkFailed("io error".into());
        assert!(err.to_string().contains("Audit sink failed"));
        assert!(err.to_string().contains("io error"));
    }

    #[test]
    fn audit_error_event_encoding_failed() {
        let err = AuditError::EventEncodingFailed("JSON serialize error".into());
        assert!(err.to_string().contains("Audit event encoding failed"));
        assert!(err.to_string().contains("JSON serialize error"));
    }

    // ========================================
    // Error Trait Tests
    // ========================================

    #[test]
    fn all_errors_implement_debug() {
        // Compile-time verification that all error types implement Debug
        fn assert_debug<T: std::fmt::Debug>() {}
        assert_debug::<CerberusError>();
        assert_debug::<RequestError>();
        assert_debug::<PolicyError>();
        assert_debug::<FilterError>();
        assert_debug::<SandboxSetupError>();
        assert_debug::<ExecutionError>();
        assert_debug::<AuditError>();
    }

    #[test]
    fn all_errors_implement_error() {
        // Compile-time verification that all error types implement std::error::Error
        fn assert_error<T: std::error::Error>() {}
        assert_error::<CerberusError>();
        assert_error::<RequestError>();
        assert_error::<PolicyError>();
        assert_error::<FilterError>();
        assert_error::<SandboxSetupError>();
        assert_error::<ExecutionError>();
        assert_error::<AuditError>();
    }

    #[test]
    fn cerberus_error_is_send_sync() {
        // Verify CerberusError can be used across thread boundaries
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<CerberusError>();
        assert_send_sync::<RequestError>();
        assert_send_sync::<PolicyError>();
        assert_send_sync::<FilterError>();
        assert_send_sync::<SandboxSetupError>();
        assert_send_sync::<ExecutionError>();
        assert_send_sync::<AuditError>();
    }
}
