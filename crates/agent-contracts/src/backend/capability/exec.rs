use crate::backend::{BackendPath, OperationError};
use async_trait::async_trait;

/// Request to execute a command.
#[derive(Debug, Clone)]
pub struct ExecRequest {
    pub command: String,
    pub args: Vec<String>,
    pub shell: Option<String>,
    pub cwd: Option<BackendPath>,
    pub timeout_ms: Option<u64>,
}

/// Result of command execution.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// Command execution capability.
#[async_trait]
pub trait OperationExec: Send + Sync {
    /// Execute a command.
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError>;
}
