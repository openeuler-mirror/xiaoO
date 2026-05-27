use crate::backends::local::backend::{file_name_string, io_error_for_path, LocalBackendState};
use agent_contracts::backend::{
    capability::{export::ExportFileRequest, OperationExport},
    ExportedFileHandle, ExportedFileMeta, ExportedFileReader, OperationError,
    SharedExportedFileHandle,
};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct LocalExport {
    _state: Arc<LocalBackendState>,
}

impl LocalExport {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

struct LocalExportedFileHandle {
    state: Arc<LocalBackendState>,
    host_path: PathBuf,
    metadata: ExportedFileMeta,
}

#[async_trait]
impl ExportedFileHandle for LocalExportedFileHandle {
    fn metadata(&self) -> &ExportedFileMeta {
        &self.metadata
    }

    async fn open_read(&self) -> Result<ExportedFileReader, OperationError> {
        self.state
            .policy
            .check_read(self.host_path.as_path(), "export")?;
        let file = tokio::fs::File::open(self.host_path.as_path())
            .await
            .map_err(|error| io_error_for_path(self.host_path.as_path(), error))?;
        Ok(Box::new(file))
    }
}

#[async_trait]
impl OperationExport for LocalExport {
    async fn export_file(
        &self,
        request: ExportFileRequest,
    ) -> Result<SharedExportedFileHandle, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        self._state
            .policy
            .check_read(host_path.as_path(), "export")?;
        self._state.ensure_file(host_path.as_path())?;
        let metadata = std::fs::metadata(host_path.as_path())
            .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
        let file_name = match request.preferred_name {
            Some(name) => name,
            None => file_name_string(host_path.as_path())?,
        };

        Ok(Arc::new(LocalExportedFileHandle {
            state: Arc::clone(&self._state),
            host_path,
            metadata: ExportedFileMeta {
                file_name,
                size_bytes: Some(metadata.len()),
                media_type: None,
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::local::backend::LocalBackendState;
    use crate::backends::local::policy::LocalBackendPolicy;
    use agent_contracts::backend::BackendPath;

    fn export(root: &std::path::Path) -> LocalExport {
        let workspace = root.join("workspace");
        let temp = workspace.join("tmp");
        std::fs::create_dir_all(temp.as_path()).unwrap();
        LocalExport::new(Arc::new(LocalBackendState {
            backend_id: "local-test".to_string(),
            workspace_root: BackendPath(workspace.display().to_string()),
            workspace_root_host: workspace.clone(),
            home_dir: None,
            home_dir_host: None,
            temp_root_host: temp.clone(),
            default_shell: None,
            policy: LocalBackendPolicy::test_macos_seatbelt(
                vec![workspace.clone(), temp.clone()],
                vec![temp],
                false,
            ),
        }))
    }

    #[test]
    fn export_outside_policy_root_is_denied() {
        let root =
            std::env::temp_dir().join(format!("xiaoo-local-export-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(root.as_path());
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        let file = root.join("outside").join("secret.txt");
        std::fs::write(file.as_path(), b"secret").unwrap();
        let export = export(root.as_path());

        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(export.export_file(ExportFileRequest {
                path: BackendPath(file.display().to_string()),
                preferred_name: None,
            }));

        assert!(matches!(
            result,
            Err(OperationError::SandboxPolicyDenied { .. })
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
    }
}
