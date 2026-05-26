use crate::backends::local::backend::{
    file_name_string, io_error_for_path, system_time_millis, LocalBackendState,
};
use agent_contracts::backend::{
    capability::{
        filesystem::{
            ReadBytesRequest, TempPathKind, TempPathRequest, WriteBytesOutcome, WriteBytesRequest,
            WriteMode,
        },
        OperationFileSystem,
    },
    BackendPath, OperationError, PathStat,
};
use async_trait::async_trait;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
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
    async fn stat(&self, path: &BackendPath) -> Result<PathStat, OperationError> {
        let host_path = self._state.backend_path_to_host(path)?;
        self._state.policy.check_read(host_path.as_path())?;
        self._state.stat_for_path(host_path.as_path())
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        self._state.policy.check_read(host_path.as_path())?;
        self._state.ensure_file(host_path.as_path())?;
        std::fs::read(host_path.as_path())
            .map_err(|error| io_error_for_path(host_path.as_path(), error))
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        self._state.policy.check_write(host_path.as_path())?;
        let parent = host_path
            .parent()
            .ok_or_else(|| OperationError::InvalidPath {
                message: format!(
                    "path does not have a parent directory: {}",
                    host_path.display()
                ),
            })?;
        self._state.policy.check_write(parent)?;
        self._state.ensure_directory(parent)?;
        let existed = host_path.exists();

        match request.mode {
            WriteMode::Create => {
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(host_path.as_path())
                    .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
                file.write_all(request.content.as_slice())
                    .map_err(|error| OperationError::Transport {
                        message: format!("{}: {error}", host_path.display()),
                    })?;
            }
            WriteMode::Overwrite => {
                let mut file = OpenOptions::new()
                    .create(true)
                    .truncate(true)
                    .write(true)
                    .open(host_path.as_path())
                    .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
                file.write_all(request.content.as_slice())
                    .map_err(|error| OperationError::Transport {
                        message: format!("{}: {error}", host_path.display()),
                    })?;
            }
            WriteMode::AtomicOverwrite => {
                let temp_path = atomic_write_temp_path(parent, host_path.as_path())?;
                self._state.policy.check_write(temp_path.as_path())?;
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(temp_path.as_path())
                    .map_err(|error| io_error_for_path(temp_path.as_path(), error))?;
                file.write_all(request.content.as_slice())
                    .map_err(|error| OperationError::Transport {
                        message: format!("{}: {error}", temp_path.display()),
                    })?;
                std::fs::rename(temp_path.as_path(), host_path.as_path())
                    .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
            }
        }

        Ok(WriteBytesOutcome {
            path: request.path,
            created: !existed,
        })
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        let host_path = self._state.backend_path_to_host(path)?;
        self._state.policy.check_write(host_path.as_path())?;
        std::fs::create_dir_all(host_path.as_path())
            .map_err(|error| io_error_for_path(host_path.as_path(), error))
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        let parent = match request.preferred_parent.as_ref() {
            Some(path) => self._state.backend_path_to_host(path)?,
            None => self._state.temp_root_host.clone(),
        };
        self._state.policy.check_write(parent.as_path())?;
        self._state.ensure_directory(parent.as_path())?;

        let generated = temp_entry_path(
            parent.as_path(),
            request.prefix.as_deref().unwrap_or("tmp-"),
            request.suffix.as_deref().unwrap_or(""),
        )?;
        self._state.policy.check_write(generated.as_path())?;

        match request.kind {
            TempPathKind::File => {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(generated.as_path())
                    .map_err(|error| io_error_for_path(generated.as_path(), error))?;
            }
            TempPathKind::Directory => {
                std::fs::create_dir(generated.as_path())
                    .map_err(|error| io_error_for_path(generated.as_path(), error))?;
            }
        }

        self._state.host_path_to_backend(generated.as_path())
    }
}

fn atomic_write_temp_path(parent: &Path, destination: &Path) -> Result<PathBuf, OperationError> {
    let name = file_name_string(destination)?;
    temp_entry_path(parent, format!(".{name}.atomic-").as_str(), ".tmp")
}

fn temp_entry_path(parent: &Path, prefix: &str, suffix: &str) -> Result<PathBuf, OperationError> {
    let timestamp = system_time_millis(std::time::SystemTime::now());
    let candidate = parent.join(format!(
        "{prefix}{}-{timestamp}{suffix}",
        std::process::id()
    ));
    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::local::backend::LocalBackendState;
    use crate::backends::local::policy::LocalBackendPolicy;

    fn test_root() -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("xiaoo-local-fs-policy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(root.as_path());
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        std::fs::create_dir_all(root.join("outside")).unwrap();
        root
    }

    fn filesystem(root: &Path) -> LocalFileSystem {
        let workspace = root.join("workspace");
        let temp = workspace.join("tmp");
        std::fs::create_dir_all(temp.as_path()).unwrap();
        LocalFileSystem::new(Arc::new(LocalBackendState {
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
    fn read_outside_policy_root_is_denied() {
        let root = test_root();
        let file = root.join("outside").join("secret.txt");
        std::fs::write(file.as_path(), b"secret").unwrap();
        let fs = filesystem(root.as_path());

        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fs.read_bytes(ReadBytesRequest {
                path: BackendPath(file.display().to_string()),
            }));

        assert!(matches!(
            result,
            Err(OperationError::PermissionDenied { .. })
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
    }

    #[test]
    fn write_outside_writable_root_is_denied() {
        let root = test_root();
        let file = root.join("workspace").join("src.txt");
        let fs = filesystem(root.as_path());

        let result = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fs.write_bytes(WriteBytesRequest {
                path: BackendPath(file.display().to_string()),
                content: b"change".to_vec(),
                mode: WriteMode::Overwrite,
            }));

        assert!(matches!(
            result,
            Err(OperationError::PermissionDenied { .. })
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
    }
}
