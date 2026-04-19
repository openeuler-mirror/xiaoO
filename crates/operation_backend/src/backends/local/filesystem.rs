use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{
        filesystem::{ReadBytesRequest, TempPathRequest, WriteBytesOutcome, WriteBytesRequest},
        OperationFileSystem,
    },
    BackendPath, OperationError, PathStat,
};
use async_trait::async_trait;
use std::sync::Arc;

pub(crate) struct LocalFileSystem {
    _state: Arc<LocalBackendState>,
}

impl LocalFileSystem {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationFileSystem for LocalFileSystem {
    async fn stat(&self, _path: &BackendPath) -> Result<PathStat, OperationError> {
        todo!("local filesystem stat is not implemented yet")
    }

    async fn read_bytes(&self, _request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        todo!("local filesystem read_bytes is not implemented yet")
    }

    async fn write_bytes(
        &self,
        _request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        todo!("local filesystem write_bytes is not implemented yet")
    }

    async fn create_dir_all(&self, _path: &BackendPath) -> Result<(), OperationError> {
        todo!("local filesystem create_dir_all is not implemented yet")
    }

    async fn temp_path(&self, _request: TempPathRequest) -> Result<BackendPath, OperationError> {
        todo!("local filesystem temp_path is not implemented yet")
    }
}
