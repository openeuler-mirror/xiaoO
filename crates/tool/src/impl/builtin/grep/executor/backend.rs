use std::path::Path;
use std::sync::Arc;

use agent_contracts::backend::capability::exec::ExecRequest;
use agent_contracts::backend::capability::path::{ResolveBase, ResolvePathRequest};
use agent_contracts::backend::{BackendPath, PathKind};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

use super::super::validation::backend as validation;
use super::constants::{DEFAULT_HEAD_LIMIT, RG_MAX_COLUMNS, VCS_DIRECTORIES_TO_EXCLUDE};
use super::input::{GrepInput, OutputMode};
use super::output::GrepOutput;
use super::spec::GrepToolSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedSearchTarget {
    cwd: BackendPath,
    search_target: String,
}

pub struct GrepExecutor {
    spec: Arc<GrepToolSpec>,
}

impl GrepExecutor {
    pub fn new(spec: Arc<GrepToolSpec>) -> Self {
        Self { spec }
    }

    fn build_rg_args(input: &GrepInput, search_target: &str) -> Vec<String> {
        let mut args = vec![
            "--hidden".to_string(),
            "--max-columns".to_string(),
            RG_MAX_COLUMNS.to_string(),
        ];

        for dir in VCS_DIRECTORIES_TO_EXCLUDE {
            args.push("--glob".to_string());
            args.push(format!("!{}", dir));
        }

        if input.multiline.unwrap_or(false) {
            args.push("-U".to_string());
            args.push("--multiline-dotall".to_string());
        }

        if input.case_insensitive.unwrap_or(false) {
            args.push("-i".to_string());
        }

        let output_mode = input.output_mode.unwrap_or(OutputMode::FilesWithMatches);
        match output_mode {
            OutputMode::FilesWithMatches => {
                args.push("-l".to_string());
            }
            OutputMode::Count => {
                args.push("--count-matches".to_string());
            }
            OutputMode::Content => {
                if input.show_line_numbers.unwrap_or(true) {
                    args.push("-n".to_string());
                }

                if let Some(ctx) = input.context {
                    args.push("-C".to_string());
                    args.push(ctx.to_string());
                } else if let Some(ctx_c) = input.context_c {
                    args.push("-C".to_string());
                    args.push(ctx_c.to_string());
                } else {
                    if let Some(before) = input.context_before {
                        args.push("-B".to_string());
                        args.push(before.to_string());
                    }
                    if let Some(after) = input.context_after {
                        args.push("-A".to_string());
                        args.push(after.to_string());
                    }
                }
            }
        }

        if input.pattern.starts_with('-') {
            args.push("-e".to_string());
            args.push(input.pattern.clone());
        } else {
            args.push(input.pattern.clone());
        }

        if let Some(ref file_type) = input.file_type {
            args.push("--type".to_string());
            args.push(file_type.clone());
        }

        if let Some(ref glob) = input.glob {
            for pattern in glob
                .split(|c| c == ',' || c == ' ')
                .filter(|s| !s.is_empty())
            {
                args.push("--glob".to_string());
                args.push(pattern.trim().to_string());
            }
        }

        args.push(search_target.to_string());
        args
    }

    fn validate_scope_options(_input: &GrepInput) -> Result<(), String> {
        Ok(())
    }

    async fn resolve_search_target(
        path: Option<&str>,
        backend: &dyn agent_contracts::backend::OperationBackend,
    ) -> Result<ResolvedSearchTarget, String> {
        match path {
            None => Ok(ResolvedSearchTarget {
                cwd: backend.paths().workspace_root().clone(),
                search_target: ".".to_string(),
            }),
            Some(path) => {
                let resolved = backend
                    .paths()
                    .resolve_path(ResolvePathRequest {
                        raw_path: path.trim().to_string(),
                        base: ResolveBase::WorkspaceRoot,
                    })
                    .await
                    .map_err(|e| format!("Failed to resolve path: {}", e))?;

                let stat = backend
                    .files()
                    .stat(&resolved)
                    .await
                    .map_err(|e| format!("Failed to stat path: {}", e))?;

                if !stat.exists {
                    return Err(format!("Path does not exist: {}", path));
                }

                match stat.kind {
                    Some(PathKind::Directory) => Ok(ResolvedSearchTarget {
                        cwd: resolved,
                        search_target: ".".to_string(),
                    }),
                    Some(PathKind::File) => {
                        let resolved_path = Path::new(resolved.0.as_str());
                        let parent = resolved_path
                            .parent()
                            .and_then(|value| value.to_str())
                            .ok_or_else(|| {
                                format!(
                                    "Failed to resolve parent directory for file path: {}",
                                    resolved.0
                                )
                            })?;
                        let file_name = resolved_path
                            .file_name()
                            .and_then(|value| value.to_str())
                            .ok_or_else(|| {
                                format!("Failed to resolve file name for path: {}", resolved.0)
                            })?;
                        Ok(ResolvedSearchTarget {
                            cwd: BackendPath(parent.to_string()),
                            search_target: file_name.to_string(),
                        })
                    }
                    _ => Err(format!("Unsupported path kind: {}", path)),
                }
            }
        }
    }

    async fn run_rg(
        backend: &dyn agent_contracts::backend::OperationBackend,
        args: Vec<String>,
        cwd: BackendPath,
    ) -> Result<Vec<String>, String> {
        let output = backend
            .exec()
            .exec(ExecRequest {
                command: "rg".to_string(),
                args,
                shell: None,
                cwd: Some(cwd),
                timeout_ms: None,
                env: None,
            })
            .await
            .map_err(|e| format!("Failed to execute rg via backend exec: {}", e))?;

        if output.exit_code == Some(0) || output.exit_code == Some(1) {
            let stdout = String::from_utf8_lossy(output.stdout.as_slice());
            return Ok(stdout.lines().map(|line| line.replace('\r', "")).collect());
        }

        Err(format!(
            "rg exited with code {:?}: {}",
            output.exit_code,
            String::from_utf8_lossy(output.stderr.as_slice())
        ))
    }

    async fn resolve_result_path(
        backend: &dyn agent_contracts::backend::OperationBackend,
        cwd: &BackendPath,
        raw_path: &str,
    ) -> Result<BackendPath, String> {
        backend
            .paths()
            .resolve_path(ResolvePathRequest {
                raw_path: raw_path.to_string(),
                base: ResolveBase::Explicit(cwd.clone()),
            })
            .await
            .map_err(|e| format!("Failed to resolve grep result path: {}", e))
    }

    async fn call_inner(
        &self,
        input: &GrepInput,
        resolved_target: &ResolvedSearchTarget,
        backend: &dyn agent_contracts::backend::OperationBackend,
    ) -> Result<GrepOutput, String> {
        let output_mode = input.output_mode.unwrap_or(OutputMode::FilesWithMatches);
        let head_limit = input.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
        let offset = input.offset.unwrap_or(0);

        let args = Self::build_rg_args(input, &resolved_target.search_target);
        let lines = Self::run_rg(backend, args, resolved_target.cwd.clone()).await?;

        match output_mode {
            OutputMode::Content => {
                let (limited_lines, applied_limit) =
                    Self::apply_head_limit(lines, head_limit, offset);
                let num_lines = limited_lines.len();

                let mut output = GrepOutput::new(OutputMode::Content)
                    .with_content(limited_lines.join("\n"), num_lines);

                if let Some(lim) = applied_limit {
                    output = output.with_limit(lim);
                }

                if offset > 0 {
                    output = output.with_offset(offset);
                }

                Ok(output)
            }
            OutputMode::Count => {
                let (limited_lines, applied_limit) =
                    Self::apply_head_limit(lines, head_limit, offset);

                let mut total_matches = 0usize;
                let mut file_count = 0usize;
                let mut content_lines = Vec::new();

                for line in &limited_lines {
                    if let Some((_, count_str)) = line.rsplit_once(':') {
                        if let Ok(count) = count_str.parse::<usize>() {
                            total_matches += count;
                            file_count += 1;
                        }
                    }
                    content_lines.push(line.clone());
                }

                let mut output = GrepOutput::new(OutputMode::Count).with_count(
                    total_matches,
                    file_count,
                    content_lines.join("\n"),
                );

                if let Some(lim) = applied_limit {
                    output = output.with_limit(lim);
                }

                if offset > 0 {
                    output = output.with_offset(offset);
                }

                Ok(output)
            }
            OutputMode::FilesWithMatches => {
                let mut files_with_mtime = Vec::new();

                for line in &lines {
                    let resolved_path =
                        Self::resolve_result_path(backend, &resolved_target.cwd, line).await?;
                    let stat = backend
                        .files()
                        .stat(&resolved_path)
                        .await
                        .map_err(|e| format!("Failed to stat grep result file: {}", e))?;
                    files_with_mtime.push((
                        line.clone(),
                        stat.modified_at
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                    ));
                }

                files_with_mtime.sort_by(|a, b| {
                    let time_cmp = b.1.cmp(&a.1);
                    if time_cmp == std::cmp::Ordering::Equal {
                        a.0.cmp(&b.0)
                    } else {
                        time_cmp
                    }
                });

                let sorted_files: Vec<String> =
                    files_with_mtime.into_iter().map(|(file, _)| file).collect();
                let (limited_files, applied_limit) =
                    Self::apply_head_limit(sorted_files, head_limit, offset);

                let num_files = limited_files.len();

                let mut output = GrepOutput::new(OutputMode::FilesWithMatches)
                    .with_files(limited_files, num_files);

                if let Some(lim) = applied_limit {
                    output = output.with_limit(lim);
                }

                if offset > 0 {
                    output = output.with_offset(offset);
                }

                Ok(output)
            }
        }
    }

    fn apply_head_limit<T>(items: Vec<T>, limit: u32, offset: u32) -> (Vec<T>, Option<u32>) {
        if limit == 0 {
            return (items.into_iter().skip(offset as usize).collect(), None);
        }

        let offset = offset as usize;
        let limit = limit as usize;
        let items_len = items.len();

        if offset >= items_len {
            return (Vec::new(), None);
        }

        let remaining = items_len - offset;
        let was_truncated = remaining > limit;
        let sliced: Vec<T> = items.into_iter().skip(offset).take(limit).collect();
        let applied_limit = if was_truncated {
            Some(limit as u32)
        } else {
            None
        };

        (sliced, applied_limit)
    }
}

impl Default for GrepExecutor {
    fn default() -> Self {
        Self::new(Arc::new(GrepToolSpec::new()))
    }
}

#[async_trait]
impl ToolExecutor for GrepExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: GrepInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        if let Err(e) = Self::validate_scope_options(&input) {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error { message: e },
            });
        }

        let validation_result = validation::validate_input(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);

            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let backend = match runtime.operation_backend() {
            Some(backend) => backend,
            None => {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: "grep requires operation backend access, but none is configured"
                            .to_string(),
                    },
                });
            }
        };

        let resolved_target = Self::resolve_search_target(input.path.as_deref(), &*backend)
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed { message: e })?;

        match self.call_inner(&input, &resolved_target, &*backend).await {
            Ok(output) => {
                let json = serde_json::to_string(&output).map_err(|e| {
                    ToolExecutionError::ExecutionFailed {
                        message: format!("Failed to serialize output: {}", e),
                    }
                })?;
                Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Success { output: json },
                })
            }
            Err(e) => Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error { message: e },
            }),
        }
    }
}
