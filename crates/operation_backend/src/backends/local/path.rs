use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{path::ResolveBase, path::ResolvePathRequest, OperationPathResolver},
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
        &self._state.workspace_root
    }

    fn home_dir(&self) -> Option<&BackendPath> {
        self._state.home_dir.as_ref()
    }

    async fn resolve_path(
        &self,
        request: ResolvePathRequest,
    ) -> Result<BackendPath, OperationError> {
        let base =
            match request.base {
                ResolveBase::WorkspaceRoot => self._state.workspace_root_host.as_path(),
                ResolveBase::HomeDir => self._state.home_dir_host.as_deref().ok_or_else(|| {
                    OperationError::Unsupported {
                        message: "home_dir is not configured".to_string(),
                    }
                })?,
                ResolveBase::Explicit(path) => {
                    let explicit = self._state.backend_path_to_host(&path)?;
                    return self._state.host_path_to_backend(
                        self._state
                            .resolve_host_path(request.raw_path.as_str(), explicit.as_path())?
                            .as_path(),
                    );
                }
            };

        let resolved = self
            ._state
            .resolve_host_path(request.raw_path.as_str(), base)?;
        self._state.host_path_to_backend(resolved.as_path())
    }
}
