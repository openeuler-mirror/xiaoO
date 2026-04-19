//! Audit module for execution observability and security monitoring.
//!
//! This module provides two complementary audit subsystems:
//!
//! # User-Space Execution Audit
//!
//! The primary audit system for tracking execution lifecycle events:
//!
//! - [`CoreExecEvent`]: Internal event type for execution lifecycle
//! - [`AuditEvent`]: Serializable event for audit logs and external storage
//! - [`ExecObserver`]: Trait for receiving execution events
//! - [`AuditSink`]: Trait for writing audit events to storage
//!
//! This system captures high-level execution events like request received,
//! execution started/completed, policy violations, etc.
//!
//! # eBPF Security Audit
//!
//! The kernel-level security monitoring system (only active with `ebpf` feature):
//!
//! - [`EbpfAuditEvent`]: Security events from eBPF monitoring
//! - [`FileAccessEvent`]: File access attempts (Landlock violations)
//! - [`SyscallEvent`]: Syscall attempts (seccomp violations)
//! - [`ForkEvent`]: Process fork events
//! - [`NetworkAccessEvent`]: Network connection attempts
//!
//! This system captures low-level security events directly from the kernel,
//! providing visibility into sandbox violations and security policy enforcement.
//!
//! # Example
//!
//! ```rust
//! use cerberus_core::audit::{ExecutionContext, LoggingObserver, MemorySink};
//! use cerberus_core::request::ExecRequest;
//!
//! let request = ExecRequest::new("ls").arg("-la");
//!
//! // Create observer with memory sink for testing
//! let sink = MemorySink::new();
//! let observer = LoggingObserver::new(Box::new(sink));
//!
//! // Create context with observer
//! let ctx = ExecutionContext::with_observer(&request, Box::new(observer));
//!
//! // Emit events during execution
//! ctx.emit_request_received();
//! // ... execute ...
//! # Ok::<(), cerberus_core::error::AuditError>(())
//! ```

mod context;
mod ebpf_types;
mod observer;
mod sink;

pub use context::{ExecutionContext, ExecutionContextBuilder, RequestId};
pub use ebpf_types::{
    BpfRawEvent, EbpfAuditEvent, FileAccessEvent, FileAccessResult, FileOperation, ForkEvent,
    NetworkAccessEvent, NetworkAccessResult, NetworkDirection, NetworkProtocol, SyscallEvent,
    SyscallResult, EVENT_EXEC, EVENT_FILE_ACCESS, EVENT_FORK, EVENT_NETWORK, EXEC_FILENAME_OFFSET,
    FILE_ACCESS_FLAGS_OFFSET, FILE_ACCESS_MODE_OFFSET, FILE_ACCESS_PATH_OFFSET,
    FILE_ACCESS_RETVAL_OFFSET, FORK_CHILD_PID_OFFSET, FORK_CLONE_FLAGS_OFFSET, FORK_COUNT_OFFSET,
    FORK_PARENT_PID_OFFSET, NETWORK_ADDRESS_OFFSET, NETWORK_DIRECTION_OFFSET, NETWORK_PORT_OFFSET,
    NETWORK_PROTOCOL_OFFSET, NETWORK_RESULT_OFFSET,
};
pub use observer::{
    CompositeObserver, ExecObserver, ExecutionMetrics, LoggingObserver, MetricsObserver,
    NoOpObserver,
};
pub use sink::{AuditSink, FileSink, MemorySink, MultiSink, NoOpSink};

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

/// Core execution event for the execution lifecycle.
///
/// This is the internal event type used throughout execution.
/// It can be converted to an `AuditEvent` for serialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoreExecEvent {
    /// Unique identifier for this execution.
    pub request_id: String,
    /// The program being executed.
    pub program: String,
    /// Arguments passed to the program.
    pub args: Vec<String>,
    /// When the event was created.
    pub timestamp: SystemTime,
    /// Working directory (if set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
}

/// Execution timeout event emitted from the core execution pipeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TimeoutEvent {
    /// Duration that was exceeded.
    pub duration: Duration,
    /// Process ID that was terminated after timing out.
    pub pid: u32,
    /// Event timestamp.
    pub timestamp: SystemTime,
}

impl CoreExecEvent {
    /// Create a new core event.
    pub fn new(request_id: impl Into<String>, program: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            program: program.into(),
            args: Vec::new(),
            timestamp: SystemTime::now(),
            cwd: None,
        }
    }

    /// Add arguments to the event.
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Set the working directory.
    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    /// Convert to an audit event.
    pub fn to_audit_event(&self, event_type: &str) -> AuditEvent {
        AuditEvent::from_core_event(self, event_type)
    }
}

/// Serializable audit event for logging and storage.
///
/// This is the external event type that can be written to sinks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unique identifier for this event.
    pub id: String,
    /// When the event occurred.
    pub timestamp: SystemTime,
    /// Type of event (e.g., "request_received", "execution_completed").
    #[serde(rename = "type")]
    pub event_type: String,
    /// Event-specific data.
    pub data: serde_json::Value,
}

impl AuditEvent {
    /// Create a new audit event.
    pub fn new(event_type: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: SystemTime::now(),
            event_type: event_type.into(),
            data,
        }
    }

    /// Create an audit event from a core event.
    pub fn from_core_event(event: &CoreExecEvent, event_type: &str) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: SystemTime::now(),
            event_type: event_type.to_string(),
            data: serde_json::json!({
                "request_id": event.request_id,
                "program": event.program,
                "args": event.args,
                "cwd": event.cwd,
            }),
        }
    }

    /// Set the event ID.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    /// Set the timestamp.
    pub fn timestamp(mut self, timestamp: SystemTime) -> Self {
        self.timestamp = timestamp;
        self
    }

    /// Convert to JSON string.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Convert to pretty JSON string.
    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Parse from JSON string.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Event types for audit logging.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    /// Request received for execution.
    RequestReceived,
    /// Execution has started.
    ExecutionStarted,
    /// Chunk of stdout received.
    StdoutChunk,
    /// Chunk of stderr received.
    StderrChunk,
    /// Execution completed successfully.
    ExecutionCompleted,
    /// Execution failed.
    ExecutionFailed,
    /// Execution timed out and the process was killed.
    ExecutionTimedOut,
    /// Filter applied (argument/env/output).
    FilterApplied,
    /// Policy violation detected.
    PolicyViolation,
    /// Sandbox setup completed.
    SandboxSetup,
}

impl EventType {
    /// Get the event type as a string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::RequestReceived => "request_received",
            Self::ExecutionStarted => "execution_started",
            Self::StdoutChunk => "stdout_chunk",
            Self::StderrChunk => "stderr_chunk",
            Self::ExecutionCompleted => "execution_completed",
            Self::ExecutionFailed => "execution_failed",
            Self::ExecutionTimedOut => "execution_timed_out",
            Self::FilterApplied => "filter_applied",
            Self::PolicyViolation => "policy_violation",
            Self::SandboxSetup => "sandbox_setup",
        }
    }
}

impl std::fmt::Display for EventType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_exec_event_new() {
        let event = CoreExecEvent::new("req-123", "echo");
        assert_eq!(event.request_id, "req-123");
        assert_eq!(event.program, "echo");
        assert!(event.args.is_empty());
        assert!(event.cwd.is_none());
    }

    #[test]
    fn core_exec_event_builder() {
        let event = CoreExecEvent::new("req-123", "ls")
            .args(["-la", "/tmp"])
            .cwd("/home");

        assert_eq!(event.args, vec!["-la", "/tmp"]);
        assert_eq!(event.cwd, Some(PathBuf::from("/home")));
    }

    #[test]
    fn core_exec_event_to_audit_event() {
        let core = CoreExecEvent::new("req-123", "echo").args(["hello"]);

        let audit = core.to_audit_event("request_received");

        assert_eq!(audit.event_type, "request_received");
        assert_eq!(audit.data["request_id"], "req-123");
        assert_eq!(audit.data["program"], "echo");
        assert_eq!(audit.data["args"], serde_json::json!(["hello"]));
    }

    #[test]
    fn audit_event_new() {
        let event = AuditEvent::new("test_event", serde_json::json!({"key": "value"}));

        assert!(!event.id.is_empty());
        assert_eq!(event.event_type, "test_event");
        assert_eq!(event.data["key"], "value");
    }

    #[test]
    fn audit_event_json_roundtrip() {
        let original = AuditEvent::new("test_event", serde_json::json!({"key": "value"}));

        let json = original.to_json().unwrap();
        let parsed = AuditEvent::from_json(&json).unwrap();

        assert_eq!(original.event_type, parsed.event_type);
        assert_eq!(original.data, parsed.data);
    }

    #[test]
    fn audit_event_to_json_pretty() {
        let event = AuditEvent::new("test", serde_json::json!({"key": "value"}));

        let json = event.to_json_pretty().unwrap();
        assert!(json.contains('\n'));
    }

    #[test]
    fn event_type_as_str() {
        assert_eq!(EventType::RequestReceived.as_str(), "request_received");
        assert_eq!(EventType::ExecutionStarted.as_str(), "execution_started");
        assert_eq!(EventType::StdoutChunk.as_str(), "stdout_chunk");
        assert_eq!(EventType::StderrChunk.as_str(), "stderr_chunk");
        assert_eq!(
            EventType::ExecutionCompleted.as_str(),
            "execution_completed"
        );
        assert_eq!(EventType::ExecutionFailed.as_str(), "execution_failed");
        assert_eq!(EventType::ExecutionTimedOut.as_str(), "execution_timed_out");
        assert_eq!(EventType::FilterApplied.as_str(), "filter_applied");
        assert_eq!(EventType::PolicyViolation.as_str(), "policy_violation");
        assert_eq!(EventType::SandboxSetup.as_str(), "sandbox_setup");
    }

    #[test]
    fn event_type_display() {
        assert_eq!(
            format!("{}", EventType::RequestReceived),
            "request_received"
        );
        assert_eq!(
            format!("{}", EventType::ExecutionCompleted),
            "execution_completed"
        );
    }

    #[test]
    fn audit_event_from_core_event() {
        let core = CoreExecEvent::new("req-456", "cat")
            .args(["file.txt"])
            .cwd("/data");

        let audit = AuditEvent::from_core_event(&core, "execution_started");

        assert_eq!(audit.event_type, "execution_started");
        assert_eq!(audit.data["request_id"], "req-456");
        assert_eq!(audit.data["program"], "cat");
        assert_eq!(audit.data["args"], serde_json::json!(["file.txt"]));
        assert_eq!(audit.data["cwd"], "/data");
    }
}
