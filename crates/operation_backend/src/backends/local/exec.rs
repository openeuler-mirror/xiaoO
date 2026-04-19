use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;

pub(crate) struct LocalExec {
    _state: Arc<LocalBackendState>,
}

impl LocalExec {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationExec for LocalExec {
    async fn exec(&self, _request: ExecRequest) -> Result<ExecResult, OperationError> {
        todo!("local exec is not implemented yet")
    }
}
