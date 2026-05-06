use crate::backends::local::backend::{io_error_for_path, LocalBackendState};
use agent_contracts::backend::{
    capability::{
        search::{GlobRequest, GrepMode, GrepRequest, GrepResult},
        OperationSearch,
    },
    BackendPath, OperationError,
};
use async_trait::async_trait;
use glob::Pattern;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(crate) struct LocalSearch {
    _state: Arc<LocalBackendState>,
}

impl LocalSearch {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationSearch for LocalSearch {
    async fn glob(&self, request: GlobRequest) -> Result<Vec<BackendPath>, OperationError> {
        let base_dir = match request.base_dir.as_ref() {
            Some(path) => self._state.backend_path_to_host(path)?,
            None => self._state.workspace_root_host.clone(),
        };
        self._state.ensure_directory(base_dir.as_path())?;
        let pattern = Pattern::new(request.pattern.as_str()).map_err(|error| {
            OperationError::InvalidPath {
                message: format!("invalid glob pattern: {error}"),
            }
        })?;

        let mut host_paths = Vec::new();
        collect_paths(
            base_dir.as_path(),
            base_dir.as_path(),
            &pattern,
            &mut host_paths,
        )?;
        host_paths.sort();

        let mut entries = Vec::new();
        for path in host_paths {
            entries.push(self._state.host_path_to_backend(path.as_path())?);
            if request.limit.is_some_and(|limit| entries.len() >= limit) {
                break;
            }
        }
        Ok(entries)
    }

    async fn grep(&self, request: GrepRequest) -> Result<GrepResult, OperationError> {
        let target_dir = self._state.backend_path_to_host(&request.base_dir)?;
        self._state.ensure_directory(target_dir.as_path())?;
        let include_pattern = match request.include.as_deref() {
            Some(pattern) => {
                Some(
                    Pattern::new(pattern).map_err(|error| OperationError::InvalidPath {
                        message: format!("invalid include pattern: {error}"),
                    })?,
                )
            }
            None => None,
        };

        let mut files = Vec::new();
        collect_files(target_dir.as_path(), &mut files)?;
        files.sort();

        let mut entries = Vec::new();

        for path in files {
            let relative = path
                .strip_prefix(target_dir.as_path())
                .ok()
                .and_then(|value| value.to_str())
                .unwrap_or("");
            if let Some(pattern) = include_pattern.as_ref() {
                if !pattern.matches(relative) && !pattern.matches_path(path.as_path()) {
                    continue;
                }
            }

            let content = std::fs::read(path.as_path())
                .map_err(|error| io_error_for_path(path.as_path(), error))?;
            let text = String::from_utf8_lossy(content.as_slice());
            let matched_lines: Vec<String> = text
                .lines()
                .filter(|line| line.contains(request.query.as_str()))
                .map(|line| line.to_string())
                .collect();

            if matched_lines.is_empty() {
                continue;
            }

            let backend_path = self._state.host_path_to_backend(path.as_path())?;

            match &request.mode {
                GrepMode::FilesWithMatches => {
                    entries.push(backend_path.0);
                    if request
                        .head_limit
                        .is_some_and(|limit| entries.len() >= limit)
                    {
                        break;
                    }
                }
                GrepMode::Content => {
                    for line in matched_lines {
                        entries.push(format!("{}:{}", backend_path.0, line));
                        if request
                            .head_limit
                            .is_some_and(|limit| entries.len() >= limit)
                        {
                            break;
                        }
                    }
                    if request
                        .head_limit
                        .is_some_and(|limit| entries.len() >= limit)
                    {
                        break;
                    }
                }
                GrepMode::Count => {
                    entries.push(format!("{}:{}", backend_path.0, matched_lines.len()));
                    if request
                        .head_limit
                        .is_some_and(|limit| entries.len() >= limit)
                    {
                        break;
                    }
                }
            }
        }

        Ok(GrepResult { entries })
    }
}

fn collect_paths(
    root: &Path,
    current: &Path,
    pattern: &Pattern,
    entries: &mut Vec<PathBuf>,
) -> Result<(), OperationError> {
    for entry in std::fs::read_dir(current).map_err(|error| io_error_for_path(current, error))? {
        let entry = entry.map_err(|error| io_error_for_path(current, error))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .ok()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if pattern.matches(relative) || pattern.matches_path(path.as_path()) {
            entries.push(path.clone());
        }
        if path.is_dir() {
            collect_paths(root, path.as_path(), pattern, entries)?;
        }
    }
    Ok(())
}

fn collect_files(current: &Path, entries: &mut Vec<PathBuf>) -> Result<(), OperationError> {
    for entry in std::fs::read_dir(current).map_err(|error| io_error_for_path(current, error))? {
        let entry = entry.map_err(|error| io_error_for_path(current, error))?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(path.as_path(), entries)?;
        } else if path.is_file() {
            entries.push(path);
        }
    }
    Ok(())
}
