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
    /// Additional data for a single invocation. This information originates
    /// from the pre-tool-call plugin; individual backends can parse and
    /// process the fields relevant to them.
    pub extra: Option<serde_json::Value>,
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

    /// Create directory and all parent directories, carrying per-invocation
    /// plugin data. The default delegates to [`Self::create_dir_all`]
    /// (ignoring `extra`); backends that can act on the plugin data — e.g.
    /// externalizing the mutation into a subprocess — override this.
    async fn create_dir_all_with_extra(
        &self,
        path: &BackendPath,
        extra: Option<&serde_json::Value>,
    ) -> Result<(), OperationError> {
        let _ = extra;
        self.create_dir_all(path).await
    }

    /// Create a temporary path (file or directory).
    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError>;
}
