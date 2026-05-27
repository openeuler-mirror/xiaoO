use crate::backends::local::backend::{LocalBackendState, LocalOperationBackend};
use agent_contracts::backend::{
    BackendPath, OperationBackend, OperationBackendBuildError, OperationBackendConfig,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalBackendOptions {
    workspace_root: String,
    home_dir: Option<String>,
    temp_root: Option<String>,
    default_shell: Option<String>,
}

pub async fn build_backend(
    config: &OperationBackendConfig,
) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
    let options: LocalBackendOptions =
        serde_json::from_value(config.options.clone()).map_err(|error| {
            OperationBackendBuildError::InvalidConfig {
                message: format!("invalid local backend options: {error}"),
            }
        })?;
    let workspace_root_host = absolute_dir("workspace_root", options.workspace_root.as_str())?;
    let workspace_root = backend_path_from_host_path(workspace_root_host.as_path())?;
    let home_dir_host = options
        .home_dir
        .as_deref()
        .map(|path| absolute_dir("home_dir", path))
        .transpose()?;
    let home_dir = home_dir_host
        .as_ref()
        .map(|path| backend_path_from_host_path(path.as_path()))
        .transpose()?;
    let temp_root_host = match options.temp_root.as_deref() {
        Some(path) => absolute_dir("temp_root", path)?,
        None => std::env::temp_dir(),
    };

    let backend = LocalOperationBackend::new(Arc::new(LocalBackendState {
        backend_id: "local".to_string(),
        workspace_root,
        workspace_root_host,
        home_dir,
        home_dir_host,
        temp_root_host,
        default_shell: options.default_shell,
    }));

    Ok(Arc::new(backend))
}

pub fn local_backend_for_workspace(
    workspace_root: PathBuf,
    home_dir: Option<PathBuf>,
    temp_root: Option<PathBuf>,
    default_shell: Option<String>,
) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
    let workspace_root_host = absolute_dir(
        "workspace_root",
        workspace_root
            .to_str()
            .ok_or_else(|| OperationBackendBuildError::InvalidConfig {
                message: format!(
                    "workspace_root is not valid utf-8: {}",
                    workspace_root.display()
                ),
            })?,
    )?;
    let workspace_root = backend_path_from_host_path(workspace_root_host.as_path())?;

    let home_dir_host = home_dir
        .map(|path| {
            let text = path
                .to_str()
                .ok_or_else(|| OperationBackendBuildError::InvalidConfig {
                    message: format!("home_dir is not valid utf-8: {}", path.display()),
                })?;
            absolute_dir("home_dir", text)
        })
        .transpose()?;
    let home_dir = home_dir_host
        .as_ref()
        .map(|path| backend_path_from_host_path(path.as_path()))
        .transpose()?;
    let temp_root_host = temp_root.unwrap_or_else(std::env::temp_dir);

    Ok(Arc::new(LocalOperationBackend::new(Arc::new(
        LocalBackendState {
            backend_id: "local".to_string(),
            workspace_root,
            workspace_root_host,
            home_dir,
            home_dir_host,
            temp_root_host,
            default_shell,
        },
    ))))
}

fn absolute_dir(
    field_name: &str,
    value: &str,
) -> Result<std::path::PathBuf, OperationBackendBuildError> {
    let path = Path::new(value);
    if !path.is_absolute() {
        return Err(OperationBackendBuildError::InvalidConfig {
            message: format!("{field_name} must be an absolute path: {value}"),
        });
    }
    let metadata =
        std::fs::metadata(path).map_err(|error| OperationBackendBuildError::InvalidConfig {
            message: format!("failed to read {field_name}: {error}"),
        })?;
    if !metadata.is_dir() {
        return Err(OperationBackendBuildError::InvalidConfig {
            message: format!("{field_name} must point to a directory: {value}"),
        });
    }
    Ok(path.to_path_buf())
}

fn backend_path_from_host_path(path: &Path) -> Result<BackendPath, OperationBackendBuildError> {
    let text = path
        .to_str()
        .ok_or_else(|| OperationBackendBuildError::InvalidConfig {
            message: format!("path is not valid utf-8: {}", path.display()),
        })?;
    Ok(BackendPath(text.to_string()))
}
