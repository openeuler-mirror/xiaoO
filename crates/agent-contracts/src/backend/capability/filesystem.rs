use crate::backend::{BackendPath, OperationError, PathStat};
use async_trait::async_trait;

/// Request to read file contents.
#[derive(Debug, Clone)]
pub struct ReadBytesRequest {
    pub path: BackendPath,
}

/// Request to write file contents.
#[derive(Debug, Clone)]
pub struct WriteBytesRequest {
    pub path: BackendPath,
    pub content: Vec<u8>,
    pub mode: WriteMode,
}

/// How to write a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Create,
    Overwrite,
    AtomicOverwrite,
}

/// Result of a write operation.
#[derive(Debug, Clone)]
pub struct WriteBytesOutcome {
    pub path: BackendPath,
    pub created: bool,
}

/// Request for a temporary path.
#[derive(Debug, Clone)]
pub struct TempPathRequest {
    pub kind: TempPathKind,
    pub preferred_parent: Option<BackendPath>,
    pub prefix: Option<String>,
    pub suffix: Option<String>,
}

/// Kind of temporary path to create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TempPathKind {
    File,
    Directory,
}

/// File system operations capability.
#[async_trait]
pub trait OperationFileSystem: Send + Sync {
    /// Get metadata for a path.
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError>;

    /// Read file contents.
    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError>;

    /// Write file contents.
    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError>;

    /// Create directory and all parent directories.
    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError>;

    /// Create a temporary path (file or directory).
    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError>;
}
