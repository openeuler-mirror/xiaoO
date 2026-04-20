use std::sync::Arc;

use crate::backend::OperationError;
use async_trait::async_trait;
use tokio::io::AsyncRead;

pub type ExportedFileReader = Box<dyn AsyncRead + Send + Unpin>;

#[derive(Debug, Clone)]
pub struct ExportedFileMeta {
    pub file_name: String,
    pub size_bytes: Option<u64>,
    pub media_type: Option<String>,
}

#[async_trait]
pub trait ExportedFileHandle: Send + Sync {
    fn metadata(&self) -> &ExportedFileMeta;

    async fn open_read(&self) -> Result<ExportedFileReader, OperationError>;
}

pub type SharedExportedFileHandle = Arc<dyn ExportedFileHandle>;
