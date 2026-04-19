use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{
        search::{GlobRequest, GrepRequest, GrepResult},
        OperationSearch,
    },
    BackendPath, OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;

pub(crate) struct LocalSearch {
    _state: Arc<LocalBackendState>,
}

impl LocalSearch {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationSearch for LocalSearch {
    async fn glob(&self, _request: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
        todo!("local search glob is not implemented yet")
    }

    async fn grep(&self, _request: GrepRequest) -> Result<GrepResult, OperationError> {
        todo!("local search grep is not implemented yet")
    }
}
