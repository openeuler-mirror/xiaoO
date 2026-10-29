use crate::backends::local::backend::{
    file_name_string, io_error_for_path, system_time_millis, LocalBackendState,
};
use crate::backends::local::externalize;
use crate::backends::local::spawn_wrapper::{spawn_wrapper_from_extra, SpawnWrapper};
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
use base64::Engine as _;
use serde_json::{json, Value};
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

    /// The spawn_wrapper instruction for this request when it asks for
    /// fs-write externalization.
    fn externalize_wrapper(extra: Option<&Value>) -> Option<SpawnWrapper> {
        spawn_wrapper_from_extra(extra).filter(|wrapper| wrapper.externalize_fs_writes)
    }

    /// The archived backend config for the envelope's backend section;
    /// ad-hoc states without one fail loud.
    fn externalize_source_config(&self) -> Result<&Value, OperationError> {
        self._state
            .source_config
            .as_ref()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "spawn_wrapper requested externalized fs writes but this backend has \
                          no archived source config to rebuild from"
                    .to_string(),
            })
    }

    async fn write_bytes_externalized(
        &self,
        request: &WriteBytesRequest,
        wrapper: &SpawnWrapper,
    ) -> Result<WriteBytesOutcome, OperationError> {
        let source_config = self.externalize_source_config()?;
        let args = json!({
            "path": request.path.native().to_string(),
            "content_b64": base64::engine::general_purpose::STANDARD.encode(&request.content),
            "mode": write_mode_tag(request.mode),
        });
        let outcome =
            externalize::run_externalized(wrapper, source_config, "write_bytes", args).await?;
        let created = outcome
            .get("created")
            .and_then(Value::as_bool)
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "externalized write_bytes result is missing the 'created' field"
                    .to_string(),
            })?;
        Ok(WriteBytesOutcome {
            path: request.path.clone(),
            created,
        })
    }
}

fn write_mode_tag(mode: WriteMode) -> &'static str {
    match mode {
        WriteMode::Create => "create",
        WriteMode::Overwrite => "overwrite",
        WriteMode::AtomicOverwrite => "atomic_overwrite",
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
        if let Some(wrapper) = Self::externalize_wrapper(request.extra.as_ref()) {
            return self.write_bytes_externalized(&request, &wrapper).await;
        }

        let host_path = self._state.backend_path_to_host(&request.path)?;
        let state = Arc::clone(&self._state);
        let mode = request.mode;
        let content = request.content;
        let returned_path = request.path;
        process_coordination::write_blocking(
            move || -> Result<WriteBytesOutcome, OperationError> {
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
            },
        )
        .await
        .map_err(|join_error| OperationError::Transport {
            message: format!("write_bytes blocking task panicked: {join_error}"),
        })?
        .ok_or_else(|| OperationError::Transport {
            message: "write admission cancelled or deadline elapsed".into(),
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

    async fn create_dir_all_with_extra(
        &self,
        path: &BackendPath,
        extra: Option<&Value>,
    ) -> Result<(), OperationError> {
        if let Some(wrapper) = Self::externalize_wrapper(extra) {
            let source_config = self.externalize_source_config()?;
            let args = json!({ "path": path.native().to_string() });
            externalize::run_externalized(&wrapper, source_config, "create_dir_all", args).await?;
            return Ok(());
        }
        self.create_dir_all(path).await
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
        let task = move || -> Result<BackendPath, OperationError> {
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
        };
        let result = match kind {
            TempPathKind::File => process_coordination::write_blocking(task).await,
            TempPathKind::Directory => tokio::task::spawn_blocking(move || Some(task())).await,
        };
        result
            .map_err(|join_error| OperationError::Transport {
                message: format!("temp_path blocking task panicked: {join_error}"),
            })?
            .ok_or_else(|| OperationError::Transport {
                message: "temp file admission cancelled or deadline elapsed".into(),
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
#[path = "../../../../../tests/unit/operation_backend/backends/local/filesystem_test.rs"]
mod tests;
