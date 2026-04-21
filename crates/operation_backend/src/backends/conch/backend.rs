use crate::backends::conch::{
    exec::ConchExec, filesystem::ConchFileSystem, path::ConchPathResolver, search::ConchSearch,
};
use agent_contracts::backend::{
    capability::{
        OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
    },
    BackendPath, OperationBackend, OperationBackendCapabilities, OperationError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

pub(crate) struct ConchBackendState {
    pub(crate) backend_id: String,
    pub(crate) workspace_root: BackendPath,
    pub(crate) home_dir: Option<BackendPath>,
    pub(crate) temp_root: BackendPath,
    pub(crate) default_shell: Option<String>,
    pub(crate) control_plane: ConchControlPlane,
    pub(crate) sandbox: ConchSandboxHandle,
    pub(crate) lifecycle: Mutex<ConchLifecycle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConchLifecycle {
    Active,
    ShuttingDown,
    Closed,
}

#[derive(Clone)]
pub(crate) struct ConchControlPlane {
    pub(crate) transport: ConchControlTransport,
    pub(crate) namespace: String,
}

#[derive(Clone)]
pub(crate) enum ConchControlTransport {
    UnixSocket(PathBuf),
    ApiUrl(String),
}

#[derive(Clone)]
pub(crate) struct ConchSandboxHandle {
    pub(crate) sandbox_id: String,
    pub(crate) ip: String,
    pub(crate) agent_port: u16,
}

pub(crate) struct ConchStartProcess {
    pub(crate) cmd: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: HashMap<String, String>,
    pub(crate) cwd: Option<String>,
    pub(crate) content: Option<String>,
}

pub(crate) struct ConchExecOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
}

pub(crate) struct ConchUploadFile {
    pub(crate) filepath: String,
    pub(crate) content: Vec<u8>,
}

pub struct ConchOperationBackend {
    backend_id: String,
    capabilities: OperationBackendCapabilities,
    paths: ConchPathResolver,
    files: ConchFileSystem,
    search: ConchSearch,
    exec: ConchExec,
}

impl ConchOperationBackend {
    pub(crate) fn new(state: Arc<ConchBackendState>) -> Self {
        Self {
            backend_id: state.backend_id.clone(),
            capabilities: OperationBackendCapabilities {
                supports_atomic_write: false,
                supports_grep: true,
                supports_export_file: true,
            },
            paths: ConchPathResolver::new(Arc::clone(&state)),
            files: ConchFileSystem::new(Arc::clone(&state)),
            search: ConchSearch::new(Arc::clone(&state)),
            exec: ConchExec::new(state),
        }
    }
}

impl ConchBackendState {
    pub(crate) fn ensure_active(&self) -> Result<(), OperationError> {
        let lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "conch backend state lock poisoned".to_string(),
            })?;
        match *lifecycle {
            ConchLifecycle::Active => Ok(()),
            ConchLifecycle::ShuttingDown => Err(OperationError::Transport {
                message: format!("conch backend {} is shutting down", self.backend_id),
            }),
            ConchLifecycle::Closed => Err(OperationError::Transport {
                message: format!("conch backend {} is already closed", self.backend_id),
            }),
        }
    }

    pub(crate) fn begin_shutdown(&self) -> Result<bool, OperationError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "conch backend state lock poisoned".to_string(),
            })?;
        match *lifecycle {
            ConchLifecycle::Active => {
                *lifecycle = ConchLifecycle::ShuttingDown;
                Ok(true)
            }
            ConchLifecycle::ShuttingDown | ConchLifecycle::Closed => Ok(false),
        }
    }

    pub(crate) fn finish_shutdown(&self) -> Result<(), OperationError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "conch backend state lock poisoned".to_string(),
            })?;
        *lifecycle = ConchLifecycle::Closed;
        Ok(())
    }

    pub(crate) fn abort_shutdown(&self) -> Result<(), OperationError> {
        let mut lifecycle = self
            .lifecycle
            .lock()
            .map_err(|_| OperationError::Transport {
                message: "conch backend state lock poisoned".to_string(),
            })?;
        if *lifecycle == ConchLifecycle::ShuttingDown {
            *lifecycle = ConchLifecycle::Active;
        }
        Ok(())
    }

    pub(crate) fn resolve_backend_path(
        &self,
        raw_path: &str,
        base: &BackendPath,
    ) -> Result<BackendPath, OperationError> {
        let candidate = Path::new(raw_path);
        if candidate.is_absolute() {
            return normalize_backend_path(candidate);
        }
        normalize_backend_path(Path::new(base.0.as_str()).join(candidate).as_path())
    }
}

pub(crate) fn normalize_backend_path(path: &Path) -> Result<BackendPath, OperationError> {
    if !path.is_absolute() {
        return Err(OperationError::InvalidPath {
            message: format!("path must be absolute: {}", path.display()),
        });
    }

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(OperationError::InvalidPath {
                        message: format!("path escapes root: {}", path.display()),
                    });
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
        }
    }

    let text = normalized
        .to_str()
        .ok_or_else(|| OperationError::InvalidPath {
            message: format!("path is not valid utf-8: {}", normalized.display()),
        })?;
    Ok(BackendPath(text.to_string()))
}

pub(crate) fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[async_trait]
impl OperationBackend for ConchOperationBackend {
    fn backend_id(&self) -> &str {
        self.backend_id.as_str()
    }

    fn capabilities(&self) -> OperationBackendCapabilities {
        self.capabilities
    }

    fn paths(&self) -> &dyn OperationPathResolver {
        &self.paths as &dyn OperationPathResolver
    }

    fn files(&self) -> &dyn OperationFileSystem {
        &self.files as &dyn OperationFileSystem
    }

    fn search(&self) -> &dyn OperationSearch {
        &self.search as &dyn OperationSearch
    }

    fn exec(&self) -> &dyn OperationExec {
        &self.exec as &dyn OperationExec
    }

    fn export(&self) -> &dyn OperationExport {
        &self.files as &dyn OperationExport
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        if !self.exec.state().begin_shutdown()? {
            return Ok(());
        }
        match crate::backends::conch::control::delete_sandbox(self.exec.state()).await {
            Ok(()) => {
                self.exec.state().finish_shutdown()?;
                Ok(())
            }
            Err(error) => {
                self.exec.state().abort_shutdown()?;
                Err(error)
            }
        }
    }
}
