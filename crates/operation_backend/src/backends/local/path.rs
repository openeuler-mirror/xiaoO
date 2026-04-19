use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{path::ResolvePathRequest, OperationPathResolver},
    BackendPath, OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;

pub(crate) struct LocalPathResolver {
    _state: Arc<LocalBackendState>,
}

impl LocalPathResolver {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationPathResolver for LocalPathResolver {
    fn workspace_root(&self) -> &BackendPath {
        todo!("local path resolver workspace_root is not implemented yet")
    }

    fn home_dir(&self) -> Option<&BackendPath> {
        todo!("local path resolver home_dir is not implemented yet")
    }

    async fn resolve_path(
        &self,
        _request: ResolvePathRequest,
    ) -> Result<BackendPath, OperationError> {
        todo!("local path resolver resolve_path is not implemented yet")
    }
}
