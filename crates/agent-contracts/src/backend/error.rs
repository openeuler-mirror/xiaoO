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

    #[error("unsupported operation: {message}")]
    Unsupported { message: String },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String },

    #[error("transport error: {message}")]
    Transport { message: String },
}
