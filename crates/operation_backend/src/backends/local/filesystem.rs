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
        // Resolve the host path on the async thread, then move it + state
        // into a blocking task so policy.check_read + symlink_metadata run
        // off the async worker (slow disks/network mounts would otherwise
        // stall every other task on the same Tokio worker).
        let host_path = self._state.backend_path_to_host(path)?;
        let state = Arc::clone(&self._state);
        tokio::task::spawn_blocking(move || {
            state.policy.check_read(host_path.as_path(), "stat")?;
            state.stat_for_path(host_path.as_path())
        })
        .await
        .map_err(|join_error| OperationError::Transport {
            message: format!("stat blocking task panicked: {join_error}"),
        })?
    }

    async fn read_bytes(&self, request: ReadBytesRequest) -> Result<Vec<u8>, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        let state = Arc::clone(&self._state);
        tokio::task::spawn_blocking(move || {
            state.policy.check_read(host_path.as_path(), "file_read")?;
            state.ensure_file(host_path.as_path())?;
            std::fs::read(host_path.as_path())
                .map_err(|error| io_error_for_path(host_path.as_path(), error))
        })
        .await
        .map_err(|join_error| OperationError::Transport {
            message: format!("read_bytes blocking task panicked: {join_error}"),
        })?
    }

    async fn write_bytes(
        &self,
        request: WriteBytesRequest,
    ) -> Result<WriteBytesOutcome, OperationError> {
        let host_path = self._state.backend_path_to_host(&request.path)?;
        let state = Arc::clone(&self._state);
        let mode = request.mode;
        let content = request.content;
        let returned_path = request.path;
        tokio::task::spawn_blocking(move || -> Result<WriteBytesOutcome, OperationError> {
            state
                .policy
                .check_write(host_path.as_path(), "file_write")?;
            let parent = host_path
                .parent()
                .ok_or_else(|| OperationError::InvalidPath {
                    message: format!(
                        "path does not have a parent directory: {}",
                        host_path.display()
                    ),
                })?;
            state.policy.check_write(parent, "file_write")?;
            state.ensure_directory(parent)?;
            let existed = host_path.exists();

            match mode {
                WriteMode::Create => {
                    let mut file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(host_path.as_path())
                        .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
                    file.write_all(content.as_slice()).map_err(|error| {
                        OperationError::Transport {
                            message: format!("{}: {error}", host_path.display()),
                        }
                    })?;
                }
                WriteMode::Overwrite => {
                    let mut file = OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .open(host_path.as_path())
                        .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
                    file.write_all(content.as_slice()).map_err(|error| {
                        OperationError::Transport {
                            message: format!("{}: {error}", host_path.display()),
                        }
                    })?;
                }
                WriteMode::AtomicOverwrite => {
                    let temp_path = atomic_write_temp_path(parent, host_path.as_path())?;
                    state
                        .policy
                        .check_write(temp_path.as_path(), "file_write")?;
                    let mut file = OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .open(temp_path.as_path())
                        .map_err(|error| io_error_for_path(temp_path.as_path(), error))?;
                    file.write_all(content.as_slice()).map_err(|error| {
                        OperationError::Transport {
                            message: format!("{}: {error}", temp_path.display()),
                        }
                    })?;
                    std::fs::rename(temp_path.as_path(), host_path.as_path())
                        .map_err(|error| io_error_for_path(host_path.as_path(), error))?;
                }
            }

            Ok(WriteBytesOutcome {
                path: returned_path,
                created: !existed,
            })
        })
        .await
        .map_err(|join_error| OperationError::Transport {
            message: format!("write_bytes blocking task panicked: {join_error}"),
        })?
    }

    async fn create_dir_all(&self, path: &BackendPath) -> Result<(), OperationError> {
        let host_path = self._state.backend_path_to_host(path)?;
        let state = Arc::clone(&self._state);
        tokio::task::spawn_blocking(move || {
            state
                .policy
                .check_write(host_path.as_path(), "create_dir_all")?;
            std::fs::create_dir_all(host_path.as_path())
                .map_err(|error| io_error_for_path(host_path.as_path(), error))
        })
        .await
        .map_err(|join_error| OperationError::Transport {
            message: format!("create_dir_all blocking task panicked: {join_error}"),
        })?
    }

    async fn temp_path(&self, request: TempPathRequest) -> Result<BackendPath, OperationError> {
        let parent = match request.preferred_parent.as_ref() {
            Some(path) => self._state.backend_path_to_host(path)?,
            None => self._state.temp_root_host.clone(),
        };
        let state = Arc::clone(&self._state);
        let prefix = request.prefix.unwrap_or_else(|| "tmp-".to_string());
        let suffix = request.suffix.unwrap_or_default();
        let kind = request.kind;
        tokio::task::spawn_blocking(move || -> Result<BackendPath, OperationError> {
            state.policy.check_write(parent.as_path(), "temp_path")?;
            state.ensure_directory(parent.as_path())?;

            let generated = temp_entry_path(parent.as_path(), prefix.as_str(), suffix.as_str())?;
            state.policy.check_write(generated.as_path(), "temp_path")?;

            match kind {
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

            state.host_path_to_backend(generated.as_path())
        })
        .await
        .map_err(|join_error| OperationError::Transport {
            message: format!("temp_path blocking task panicked: {join_error}"),
        })?
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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ROOT: AtomicU64 = AtomicU64::new(0);

    fn test_root() -> PathBuf {
        let sequence = NEXT_TEST_ROOT.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "xiaoo-local-fs-policy-{}-{sequence}",
            std::process::id()
        ));
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
            Err(OperationError::SandboxPolicyDenied { .. })
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
            Err(OperationError::SandboxPolicyDenied { .. })
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
    }
}
