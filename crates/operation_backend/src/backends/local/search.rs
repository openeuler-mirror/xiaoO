use crate::backends::local::backend::{io_error_for_path, LocalBackendState};
use agent_contracts::backend::{
    capability::{
        search::{GlobRequest, GrepEntry, GrepMode, GrepPatternKind, GrepRequest, GrepResult},
        OperationSearch,
    },
    BackendPath, OperationError,
};
use async_trait::async_trait;
use glob::Pattern;
use regex::Regex;
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

/// Pattern matcher abstraction that supports both literal and regex matching.
enum PatternMatcher {
    Literal {
        pattern: String,
        case_insensitive: bool,
    },
    Regex(Regex),
}

impl PatternMatcher {
    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Literal {
                pattern,
                case_insensitive,
            } => {
                if *case_insensitive {
                    line.to_lowercase().contains(&pattern.to_lowercase())
                } else {
                    line.contains(pattern.as_str())
                }
            }
            Self::Regex(re) => re.is_match(line),
        }
    }
}

/// Build a `PatternMatcher` from the grep request fields.
fn build_matcher(request: &GrepRequest) -> Result<PatternMatcher, OperationError> {
    match request.pattern_kind {
        GrepPatternKind::Regex => {
            let pattern = if request.case_insensitive {
                format!("(?i){}", request.pattern)
            } else {
                request.pattern.clone()
            };
            let re = Regex::new(&pattern).map_err(|e| OperationError::Unsupported {
                message: format!("regex compilation failed: {e}"),
            })?;
            Ok(PatternMatcher::Regex(re))
        }
        GrepPatternKind::Literal => Ok(PatternMatcher::Literal {
            pattern: request.pattern.clone(),
            case_insensitive: request.case_insensitive,
        }),
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
        let matcher = build_matcher(&request)?;

        let target_dir = self._state.backend_path_to_host(&request.target)?;
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
        collect_files(target_dir.as_path(), &mut files, request.exclude_vcs)?;
        files.sort();

        let files_searched = files.len();
        let skip = request.offset.unwrap_or(0);
        let mut entries = Vec::new();
        let mut truncated = false;
        let mut seen = 0usize;

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
            let all_lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();

            // Find matching line indices (0-based)
            let matched_indices: Vec<usize> = all_lines
                .iter()
                .enumerate()
                .filter(|(_, line)| matcher.is_match(line.as_str()))
                .map(|(i, _)| i)
                .collect();

            if matched_indices.is_empty() {
                continue;
            }

            let backend_path = self._state.host_path_to_backend(path.as_path())?;

            match &request.mode {
                GrepMode::FilesWithMatches => {
                    seen += 1;
                    if seen <= skip {
                        continue;
                    }
                    entries.push(GrepEntry {
                        path: backend_path,
                        line_number: None,
                        line: None,
                        match_count: None,
                    });
                    if request
                        .head_limit
                        .is_some_and(|limit| entries.len() >= limit)
                    {
                        truncated = true;
                        break;
                    }
                }
                GrepMode::Content {
                    show_line_numbers,
                    context_before,
                    context_after,
                } => {
                    let ctx_before = context_before.unwrap_or(0);
                    let ctx_after = context_after.unwrap_or(0);
                    let total_lines = all_lines.len();
                    let mut visible_matches = 0usize;

                    for &match_idx in &matched_indices {
                        seen += 1;
                        if seen <= skip {
                            continue;
                        }
                        visible_matches += 1;

                        // Context lines before the match
                        let ctx_start = match_idx.saturating_sub(ctx_before);
                        for line_idx in ctx_start..match_idx {
                            entries.push(GrepEntry {
                                path: backend_path.clone(),
                                line_number: if *show_line_numbers {
                                    Some(line_idx + 1)
                                } else {
                                    None
                                },
                                line: Some(all_lines[line_idx].clone()),
                                match_count: None,
                            });
                        }

                        // The matching line itself
                        entries.push(GrepEntry {
                            path: backend_path.clone(),
                            line_number: if *show_line_numbers {
                                Some(match_idx + 1)
                            } else {
                                None
                            },
                            line: Some(all_lines[match_idx].clone()),
                            match_count: None,
                        });

                        // Context lines after the match
                        let ctx_end = (match_idx + ctx_after + 1).min(total_lines);
                        for line_idx in (match_idx + 1)..ctx_end {
                            entries.push(GrepEntry {
                                path: backend_path.clone(),
                                line_number: if *show_line_numbers {
                                    Some(line_idx + 1)
                                } else {
                                    None
                                },
                                line: Some(all_lines[line_idx].clone()),
                                match_count: None,
                            });
                        }

                        if request
                            .head_limit
                            .is_some_and(|limit| visible_matches >= limit)
                        {
                            truncated = true;
                            break;
                        }
                    }
                    if truncated {
                        break;
                    }
                }
                GrepMode::Count => {
                    seen += 1;
                    if seen <= skip {
                        continue;
                    }
                    entries.push(GrepEntry {
                        path: backend_path,
                        line_number: None,
                        line: None,
                        match_count: Some(matched_indices.len()),
                    });
                    if request
                        .head_limit
                        .is_some_and(|limit| entries.len() >= limit)
                    {
                        truncated = true;
                        break;
                    }
                }
            }
        }

        Ok(GrepResult {
            entries,
            truncated,
            files_searched,
        })
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

fn collect_files(
    current: &Path,
    entries: &mut Vec<PathBuf>,
    exclude_vcs: bool,
) -> Result<(), OperationError> {
    for entry in std::fs::read_dir(current).map_err(|error| io_error_for_path(current, error))? {
        let entry = entry.map_err(|error| io_error_for_path(current, error))?;
        let path = entry.path();
        if path.is_dir() {
            if exclude_vcs {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name == ".git" || name == ".hg" || name == ".svn" {
                        continue;
                    }
                }
            }
            collect_files(path.as_path(), entries, exclude_vcs)?;
        } else if path.is_file() {
            entries.push(path);
        }
    }
    Ok(())
}
