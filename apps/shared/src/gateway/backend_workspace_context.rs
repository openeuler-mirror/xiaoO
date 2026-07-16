use crate::gateway::workspace_prompt::{
    render_repo_map, render_workspace_prompt_block, repo_map_is_source, repo_map_path_is_skipped,
    repo_map_signatures, RepoMapEntry, WorkspacePromptFile, REPO_MAP_MAX_DEPTH, REPO_MAP_MAX_FILES,
    REPO_MAP_MAX_FILE_BYTES, REPO_MAP_MAX_VISIT,
};
use crate::gateway::SessionServiceError;
use agent_contracts::backend::{
    capability::{filesystem::ReadBytesRequest, search::GlobRequest},
    BackendPath, OperationBackend, OperationError, PathKind,
};
use std::path::{Path, PathBuf};

pub(crate) async fn compose_backend_workspace_section(
    backend: &dyn OperationBackend,
    workspace_root: &Path,
) -> Result<String, SessionServiceError> {
    let mut sections = Vec::new();
    let agents_path = workspace_root.join("AGENTS.md");
    if let Some(content) = read_optional_backend_utf8(backend, &agents_path).await? {
        let content = content.trim();
        if !content.is_empty() {
            let prompt_files = [WorkspacePromptFile {
                path: agents_path,
                content: content.to_string(),
            }];
            if let Some(block) = render_workspace_prompt_block(
                &prompt_files,
                "The following instructions were loaded from the runtime workspace root.",
            ) {
                sections.push(block);
            }
        }
    }
    if let Some(repo_map) = compose_backend_repo_map(backend, workspace_root).await? {
        sections.push(repo_map);
    }
    Ok(sections.join("\n\n"))
}

async fn compose_backend_repo_map(
    backend: &dyn OperationBackend,
    workspace_root: &Path,
) -> Result<Option<String>, SessionServiceError> {
    let candidates = backend
        .search()
        .glob(GlobRequest {
            pattern: "*".to_string(),
            base_dir: Some(BackendPath(workspace_root.display().to_string())),
            limit: Some(REPO_MAP_MAX_VISIT),
        })
        .await
        .map_err(|error| SessionServiceError::RuntimeBuild {
            message: format!("failed to enumerate backend workspace for repo map: {error}"),
        })?;

    let mut relative_paths = candidates
        .into_iter()
        .filter_map(|candidate| relative_backend_path(workspace_root, &candidate))
        .filter(|relative| relative.components().count() <= REPO_MAP_MAX_DEPTH + 1)
        .filter(|relative| {
            !repo_map_path_is_skipped(relative) && repo_map_is_source(relative.as_path())
        })
        .collect::<Vec<_>>();
    relative_paths.sort();
    relative_paths.dedup();

    let mut entries = Vec::new();
    for relative_path in relative_paths {
        if entries.len() >= REPO_MAP_MAX_FILES {
            break;
        }
        let remote_path = workspace_root.join(&relative_path);
        let stat = backend
            .files()
            .stat(&BackendPath(remote_path.display().to_string()))
            .await
            .map_err(|error| SessionServiceError::RuntimeBuild {
                message: format!(
                    "failed to stat backend repo-map file {}: {error}",
                    remote_path.display()
                ),
            })?;
        if stat.kind != Some(PathKind::File) {
            continue;
        }
        let signatures = if stat.size_bytes.unwrap_or(u64::MAX) <= REPO_MAP_MAX_FILE_BYTES {
            read_backend_utf8(backend, &remote_path)
                .await
                .ok()
                .map(|content| repo_map_signatures(&content))
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        entries.push(RepoMapEntry {
            relative_path,
            signatures,
        });
    }

    Ok(render_repo_map(entries))
}

fn relative_backend_path(workspace_root: &Path, candidate: &BackendPath) -> Option<PathBuf> {
    let candidate = Path::new(&candidate.0);
    let relative = if candidate.is_absolute() {
        candidate.strip_prefix(workspace_root).ok()?
    } else {
        candidate
    };
    (!relative.as_os_str().is_empty()).then(|| relative.to_path_buf())
}

pub(crate) async fn read_optional_backend_utf8(
    backend: &dyn OperationBackend,
    path: &Path,
) -> Result<Option<String>, SessionServiceError> {
    match backend
        .files()
        .read_bytes(ReadBytesRequest {
            path: BackendPath(path.display().to_string()),
        })
        .await
    {
        Ok(bytes) => {
            String::from_utf8(bytes)
                .map(Some)
                .map_err(|error| SessionServiceError::RuntimeBuild {
                    message: format!("backend file is not UTF-8 {}: {error}", path.display()),
                })
        }
        Err(OperationError::NotFound { .. }) => Ok(None),
        Err(error) => Err(SessionServiceError::RuntimeBuild {
            message: format!("failed to read backend file {}: {error}", path.display()),
        }),
    }
}

pub(crate) async fn read_backend_utf8(
    backend: &dyn OperationBackend,
    path: &Path,
) -> Result<String, SessionServiceError> {
    read_optional_backend_utf8(backend, path)
        .await?
        .ok_or_else(|| SessionServiceError::RuntimeBuild {
            message: format!("required backend file is missing: {}", path.display()),
        })
}
