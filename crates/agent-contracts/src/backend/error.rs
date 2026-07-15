/// What is known about a command when its execution transport is interrupted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// The command request was rejected before execution started.
    NotStarted,
    /// The command may still be running or may already have completed remotely.
    RunningOrCompleted,
}

impl std::fmt::Display for ExecutionState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotStarted => formatter.write_str("not_started"),
            Self::RunningOrCompleted => formatter.write_str("running_or_completed"),
        }
    }
}

/// Errors that can occur during backend operations.
#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("invalid path: {message}")]
    InvalidPath { message: String },

    #[error("not found: {path}")]
    NotFound { path: String },

    #[error("already exists: {path}")]
    AlreadyExists { path: String },

    #[error("not a directory: {path}")]
    NotDirectory { path: String },

    #[error("not a file: {path}")]
    NotFile { path: String },

    #[error("permission denied: {path}")]
    PermissionDenied { path: String },

    #[error("sandbox policy denied: {denial}")]
    SandboxPolicyDenied { denial: super::SandboxPolicyDenial },

    #[error("unsupported operation: {message}")]
    Unsupported { message: String },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("execution interrupted ({state}): {message}")]
    ExecutionInterrupted {
        message: String,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
        state: ExecutionState,
    },

    #[error("transport error: {message}")]
    Transport { message: String },
}
