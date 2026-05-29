use crate::backends::local::{
    exec::LocalExec, export::LocalExport, filesystem::LocalFileSystem, path::LocalPathResolver,
    search::LocalSearch,
};
use agent_contracts::backend::{
    capability::{
        OperationExec, OperationExport, OperationFileSystem, OperationPathResolver, OperationSearch,
    },
    BackendPath, OperationBackend, OperationBackendCapabilities, OperationError, PathKind,
    PathStat,
};
use async_trait::async_trait;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub(crate) struct LocalBackendState {
    pub(crate) backend_id: String,
    pub(crate) workspace_root: BackendPath,
    pub(crate) workspace_root_host: PathBuf,
    pub(crate) home_dir: Option<BackendPath>,
    pub(crate) home_dir_host: Option<PathBuf>,
    pub(crate) temp_root_host: PathBuf,
    pub(crate) default_shell: Option<String>,
}

pub struct LocalOperationBackend {
    backend_id: String,
    capabilities: OperationBackendCapabilities,
    paths: LocalPathResolver,
    files: LocalFileSystem,
    search: LocalSearch,
    exec: LocalExec,
    export: LocalExport,
}

impl LocalOperationBackend {
    /// Build a backend rooted at the process home directory.
    /// Falls back to the temp directory when HOME is unset.
    pub fn new_with_home() -> Self {
        let home_dir_host: Option<std::path::PathBuf> = std::env::var("HOME")
            .ok()
            .map(std::path::PathBuf::from)
            .or_else(|| {
                #[cfg(windows)]
                {
                    std::env::var("USERPROFILE")
                        .ok()
                        .map(std::path::PathBuf::from)
                }
                #[cfg(not(windows))]
                {
                    None
                }
            });

        let workspace_root_host = home_dir_host.clone().unwrap_or_else(std::env::temp_dir);
        let workspace_root = BackendPath(workspace_root_host.to_string_lossy().into_owned());
        let home_dir = home_dir_host
            .as_ref()
            .map(|p| BackendPath(p.to_string_lossy().into_owned()));

        Self::new(Arc::new(LocalBackendState {
            backend_id: "lsp-local".to_string(),
            workspace_root,
            workspace_root_host,
            home_dir,
            home_dir_host,
            temp_root_host: std::env::temp_dir(),
            default_shell: None,
        }))
    }

    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self {
            backend_id: state.backend_id.clone(),
            capabilities: OperationBackendCapabilities {
                supports_atomic_write: true,
                supports_grep: true,
                supports_export_file: true,
                supports_lsp: true,
            },
            paths: LocalPathResolver::new(Arc::clone(&state)),
            files: LocalFileSystem::new(Arc::clone(&state)),
            search: LocalSearch::new(Arc::clone(&state)),
            exec: LocalExec::new(Arc::clone(&state)),
            export: LocalExport::new(state),
        }
    }
}

impl LocalBackendState {
    pub(crate) fn backend_path_to_host(
        &self,
        path: &BackendPath,
    ) -> Result<PathBuf, OperationError> {
        normalize_absolute_host_path(Path::new(path.0.as_str()))
    }

    pub(crate) fn host_path_to_backend(&self, path: &Path) -> Result<BackendPath, OperationError> {
        let normalized = normalize_absolute_host_path(path)?;
        let text = normalized
            .to_str()
            .ok_or_else(|| OperationError::InvalidPath {
                message: format!("path is not valid utf-8: {}", normalized.display()),
            })?;
        Ok(BackendPath(text.to_string()))
    }

    pub(crate) fn resolve_host_path(
        &self,
        raw_path: &str,
        base: &Path,
    ) -> Result<PathBuf, OperationError> {
        if raw_path == "~" || raw_path.starts_with("~/") {
            let home_dir =
                self.home_dir_host
                    .as_ref()
                    .ok_or_else(|| OperationError::Unsupported {
                        message: "home_dir is not configured".to_string(),
                    })?;
            let suffix = raw_path.strip_prefix("~/").unwrap_or_default();
            return normalize_absolute_host_path(home_dir.join(suffix).as_path());
        }

        let candidate = Path::new(raw_path);
        let joined = if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            base.join(candidate)
        };
        normalize_absolute_host_path(joined.as_path())
    }

    pub(crate) fn stat_for_path(&self, path: &Path) -> Result<PathStat, OperationError> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) => Ok(PathStat {
                exists: true,
                kind: Some(path_kind_from_metadata(&metadata)),
                size_bytes: Some(metadata.len()),
                modified_at: metadata.modified().ok(),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PathStat {
                exists: false,
                kind: None,
                size_bytes: None,
                modified_at: None,
            }),
            Err(error) => Err(io_error_for_path(path, error)),
        }
    }

    pub(crate) fn ensure_directory(&self, path: &Path) -> Result<(), OperationError> {
        let metadata = std::fs::metadata(path).map_err(|error| io_error_for_path(path, error))?;
        if metadata.is_dir() {
            return Ok(());
        }
        Err(OperationError::NotDirectory {
            path: path.display().to_string(),
        })
    }

    pub(crate) fn ensure_file(&self, path: &Path) -> Result<(), OperationError> {
        let metadata = std::fs::metadata(path).map_err(|error| io_error_for_path(path, error))?;
        if metadata.is_file() {
            return Ok(());
        }
        Err(OperationError::NotFile {
            path: path.display().to_string(),
        })
    }
}

pub(crate) fn normalize_absolute_host_path(path: &Path) -> Result<PathBuf, OperationError> {
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

    if !normalized.is_absolute() {
        return Err(OperationError::InvalidPath {
            message: format!("path must remain absolute: {}", path.display()),
        });
    }

    Ok(normalized)
}

pub(crate) fn io_error_for_path(path: &Path, error: std::io::Error) -> OperationError {
    match error.kind() {
        std::io::ErrorKind::NotFound => OperationError::NotFound {
            path: path.display().to_string(),
        },
        std::io::ErrorKind::AlreadyExists => OperationError::AlreadyExists {
            path: path.display().to_string(),
        },
        std::io::ErrorKind::PermissionDenied => OperationError::PermissionDenied {
            path: path.display().to_string(),
        },
        _ => OperationError::Transport {
            message: format!("{}: {error}", path.display()),
        },
    }
}

pub(crate) fn file_name_string(path: &Path) -> Result<String, OperationError> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| OperationError::InvalidPath {
            message: format!(
                "path does not contain a valid file name: {}",
                path.display()
            ),
        })
}

pub(crate) fn system_time_millis(value: SystemTime) -> u128 {
    value
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn path_kind_from_metadata(metadata: &std::fs::Metadata) -> PathKind {
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        PathKind::Directory
    } else if file_type.is_file() {
        PathKind::File
    } else if file_type.is_symlink() {
        PathKind::Symlink
    } else {
        PathKind::Other
    }
}

#[async_trait]
impl OperationBackend for LocalOperationBackend {
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
        &self.export as &dyn OperationExport
    }

    async fn shutdown(&self) -> Result<(), OperationError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(home_dir_host: Option<PathBuf>) -> LocalBackendState {
        let workspace_root_host = std::env::current_dir().expect("current dir");
        let workspace_root = BackendPath(workspace_root_host.to_string_lossy().into_owned());
        let home_dir = home_dir_host
            .as_ref()
            .map(|path| BackendPath(path.to_string_lossy().into_owned()));

        LocalBackendState {
            backend_id: "test".to_string(),
            workspace_root,
            workspace_root_host,
            home_dir,
            home_dir_host,
            temp_root_host: std::env::temp_dir(),
            default_shell: None,
        }
    }

    #[test]
    fn resolves_tilde_paths_against_home_dir() {
        let home = std::env::current_dir().expect("current dir").join("home");
        let state = test_state(Some(home.clone()));
        let resolved = state
            .resolve_host_path("~/.xiaoo/tools/md_to_html.mjs", Path::new("/workspace"))
            .expect("resolve");

        assert_eq!(resolved, home.join(".xiaoo/tools/md_to_html.mjs"));
    }

    #[test]
    fn tilde_requires_configured_home_dir() {
        let state = test_state(None);
        let error = state
            .resolve_host_path("~/missing", Path::new("/workspace"))
            .expect_err("tilde should require home");

        assert!(matches!(error, OperationError::Unsupported { .. }));
    }
}
