//! Execution context for tracking request lifecycle.
//!
//! Provides a context object that holds request metadata and optional
//! observer for event emission.

use crate::error::AuditError;
use crate::request::ExecRequest;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use uuid::Uuid;

use super::{CoreExecEvent, ExecObserver, NoOpObserver, TimeoutEvent};

/// Unique identifier for an execution context.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RequestId(String);

impl RequestId {
    /// Generate a new unique request ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Create a request ID from a string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Get the request ID as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for RequestId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Context for tracking a single execution.
///
/// Holds metadata about the execution and provides methods to emit
/// events to an optional observer.
pub struct ExecutionContext {
    /// Unique request identifier.
    request_id: RequestId,
    /// The program being executed.
    program: String,
    /// Arguments for the execution.
    args: Vec<String>,
    /// Working directory (if set).
    cwd: Option<PathBuf>,
    /// When the context was created.
    created_at: SystemTime,
    /// When execution started (set when start() is called).
    started_at: Option<Instant>,
    /// Optional observer for events.
    observer: Arc<Box<dyn ExecObserver>>,
}

impl ExecutionContext {
    /// Create a new execution context from a request.
    pub fn new(request: &ExecRequest) -> Self {
        Self {
            request_id: RequestId::new(),
            program: request.program.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            created_at: SystemTime::now(),
            started_at: None,
            observer: Arc::new(Box::new(NoOpObserver::new())),
        }
    }

    /// Create a new execution context with a custom observer.
    pub fn with_observer(request: &ExecRequest, observer: Box<dyn ExecObserver>) -> Self {
        Self {
            request_id: RequestId::new(),
            program: request.program.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            created_at: SystemTime::now(),
            started_at: None,
            observer: Arc::new(observer),
        }
    }

    /// Create a new execution context with a specific request ID.
    pub fn with_id(request: &ExecRequest, id: impl Into<String>) -> Self {
        Self {
            request_id: RequestId::from_string(id),
            program: request.program.clone(),
            args: request.args.clone(),
            cwd: request.cwd.clone(),
            created_at: SystemTime::now(),
            started_at: None,
            observer: Arc::new(Box::new(NoOpObserver::new())),
        }
    }

    /// Get the request ID.
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Get the program name.
    pub fn program(&self) -> &str {
        &self.program
    }

    /// Get the arguments.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Get the working directory.
    pub fn cwd(&self) -> Option<&PathBuf> {
        self.cwd.as_ref()
    }

    /// Get the creation timestamp.
    pub fn created_at(&self) -> SystemTime {
        self.created_at
    }

    /// Check if execution has started.
    pub fn is_started(&self) -> bool {
        self.started_at.is_some()
    }

    /// Get elapsed time since context creation.
    pub fn elapsed_since_creation(&self) -> std::time::Duration {
        self.created_at
            .elapsed()
            .unwrap_or(std::time::Duration::ZERO)
    }

    /// Get elapsed time since execution started.
    pub fn elapsed_since_start(&self) -> Option<std::time::Duration> {
        self.started_at.map(|t| t.elapsed())
    }

    /// Create a core event from this context.
    pub fn to_event(&self) -> CoreExecEvent {
        CoreExecEvent {
            request_id: self.request_id.to_string(),
            program: self.program.clone(),
            args: self.args.clone(),
            timestamp: SystemTime::now(),
            cwd: self.cwd.clone(),
        }
    }

    /// Emit a request received event.
    pub fn emit_request_received(&self) -> Result<(), AuditError> {
        let event = self.to_event();
        self.observer.on_request_received(&event)
    }

    /// Start the execution (emits started event).
    pub fn start(&mut self) -> Result<(), AuditError> {
        self.started_at = Some(Instant::now());
        let event = self.to_event();
        self.observer.on_execution_started(&event)
    }

    /// Emit an output chunk event.
    pub fn emit_output_chunk(&self, chunk: &[u8], is_stderr: bool) -> Result<(), AuditError> {
        let event = self.to_event();
        self.observer.on_output_chunk(&event, chunk, is_stderr)
    }

    /// Emit stdout chunk event.
    pub fn emit_stdout(&self, chunk: &[u8]) -> Result<(), AuditError> {
        self.emit_output_chunk(chunk, false)
    }

    /// Emit stderr chunk event.
    pub fn emit_stderr(&self, chunk: &[u8]) -> Result<(), AuditError> {
        self.emit_output_chunk(chunk, true)
    }

    /// Emit execution completed event.
    pub fn emit_completed(&self, result: &crate::result::ExecResult) -> Result<(), AuditError> {
        let event = self.to_event();
        self.observer.on_execution_completed(&event, result)
    }

    /// Emit execution timed out event.
    pub fn emit_timeout(&self, timeout: &TimeoutEvent) -> Result<(), AuditError> {
        let event = self.to_event();
        self.observer.on_execution_timed_out(&event, timeout)
    }

    /// Emit execution failed event.
    pub fn emit_failed(&self, error: &str) -> Result<(), AuditError> {
        let event = self.to_event();
        self.observer.on_execution_failed(&event, error)
    }
}

impl Clone for ExecutionContext {
    fn clone(&self) -> Self {
        Self {
            request_id: self.request_id.clone(),
            program: self.program.clone(),
            args: self.args.clone(),
            cwd: self.cwd.clone(),
            created_at: self.created_at,
            started_at: self.started_at,
            observer: Arc::clone(&self.observer),
        }
    }
}

/// Builder for creating execution contexts with custom configuration.
pub struct ExecutionContextBuilder {
    request: ExecRequest,
    id: Option<String>,
    observer: Option<Box<dyn ExecObserver>>,
}

impl ExecutionContextBuilder {
    /// Create a new builder from a request.
    pub fn new(request: ExecRequest) -> Self {
        Self {
            request,
            id: None,
            observer: None,
        }
    }

    /// Set a custom request ID.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the observer.
    pub fn observer(mut self, observer: Box<dyn ExecObserver>) -> Self {
        self.observer = Some(observer);
        self
    }

    /// Build the execution context.
    pub fn build(self) -> ExecutionContext {
        let mut ctx = if let Some(observer) = self.observer {
            ExecutionContext::with_observer(&self.request, observer)
        } else {
            ExecutionContext::new(&self.request)
        };

        if let Some(id) = self.id {
            ctx.request_id = RequestId::from_string(id);
        }

        ctx
    }
}
