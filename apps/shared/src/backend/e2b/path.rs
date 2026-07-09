use agent_contracts::backend::{
    capability::{path::ResolveBase, path::ResolvePathRequest, OperationPathResolver},
    BackendPath, OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;

use super::backend::E2bBackendState;

pub(crate) struct E2bPathResolver {
    state: Arc<E2bBackendState>,
}

impl E2bPathResolver {
    pub(crate) fn new(state: Arc<E2bBackendState>) -> Self {
        Self { state }
    }
}

#[async_trait]
impl OperationPathResolver for E2bPathResolver {
    fn workspace_root(&self) -> &BackendPath {
        &self.state.workspace_root
    }

    fn home_dir(&self) -> Option<&BackendPath> {
        self.state.home_dir.as_ref()
    }

    async fn resolve_path(
        &self,
        request: ResolvePathRequest,
    ) -> Result<BackendPath, OperationError> {
        self.state.ensure_active()?;
        let base = match request.base {
            ResolveBase::WorkspaceRoot => &self.state.workspace_root,
            ResolveBase::HomeDir => {
                self.state
                    .home_dir
                    .as_ref()
                    .ok_or_else(|| OperationError::Unsupported {
                        message: "home_dir is not configured".to_string(),
                    })?
            }
            ResolveBase::Explicit(path) => {
                return self
                    .state
                    .resolve_backend_path(request.raw_path.as_str(), &path);
            }
        };
        self.state
            .resolve_backend_path(request.raw_path.as_str(), base)
    }
}
