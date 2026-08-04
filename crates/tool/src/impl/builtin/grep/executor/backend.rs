use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use agent_contracts::backend::capability::exec::ExecRequest;
use agent_contracts::backend::capability::path::{ResolveBase, ResolvePathRequest};
use agent_contracts::backend::{BackendPath, PathKind};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

use super::super::validation::backend as validation;
use super::constants::{
    default_timeout_ms, DEFAULT_HEAD_LIMIT, RG_MAX_COLUMNS, VCS_DIRECTORIES_TO_EXCLUDE,
};
use super::input::{GrepInput, OutputMode};
use super::output::GrepOutput;
use super::spec::GrepToolSpec;
use crate::r#impl::fs_timeout::{timed, DEFAULT_FS_TIMEOUT_MS};

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedSearchTarget {
    cwd: BackendPath,
    search_target: String,
}

/// Max concurrent `stat` calls when sorting `FilesWithMatches` by mtime.
/// Each `stat` runs on the Tokio blocking pool; bounding parallelism avoids
/// flooding it when the match set is large (e.g. thousands of files).
const STAT_CONCURRENCY_LIMIT: usize = 64;

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
                let resolved = timed(
                    "grep resolve_path",
                    DEFAULT_FS_TIMEOUT_MS,
                    backend.paths().resolve_path(ResolvePathRequest {
                        raw_path: path.trim().to_string(),
                        base: ResolveBase::WorkspaceRoot,
                    }),
                )
                .await
                .map_err(|e| format!("Failed to resolve path: {}", e))?;

                let stat = timed(
                    "grep stat",
                    DEFAULT_FS_TIMEOUT_MS,
                    backend.files().stat(&resolved),
                )
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
        timeout_ms: u64,
    ) -> Result<Vec<String>, String> {
        let output = backend
            .exec()
            .exec(ExecRequest {
                command: "rg".to_string(),
                args,
                shell: None,
                cwd: Some(cwd),
                timeout_ms: Some(timeout_ms),
                env: None,
            })
            .await
            .map_err(|e| format!("Failed to execute rg via backend exec: {}", e))?;

        if output.exit_code == Some(0) || output.exit_code == Some(1) {
            let stdout = String::from_utf8_lossy(output.stdout.as_slice());
            return Ok(stdout.lines().map(|line| line.replace('\r', "")).collect());
        }

        if output.timed_out {
            return Err(format!(
                "rg timed out after {}ms; narrow the search path, tighten the pattern, or pass a larger `timeout`",
                timeout_ms
            ));
        }

        Err(format!(
            "rg exited with code {:?}: {}",
            output.exit_code,
            String::from_utf8_lossy(output.stderr.as_slice())
        ))
    }

    /// Build GNU `grep` arguments mirroring [`Self::build_rg_args`].
    ///
    /// Used as a fallback when the `rg` binary is unavailable in the execution
    /// environment (e.g. SWE-bench containers ship `grep` but not ripgrep).
    /// `-P` selects the PCRE engine, the closest match to ripgrep's regex
    /// flavor (`\d`, `\w`, `\s`, lazy quantifiers). Multiline and `--type`
    /// have no clean grep equivalent and are intentionally dropped — degrading
    /// to a superset of results is safer than hard-failing the whole call.
    fn build_grep_args(input: &GrepInput, search_target: &str) -> Vec<String> {
        let recursive = search_target == ".";
        let mut args = vec!["-P".to_string()];

        if recursive {
            args.push("-r".to_string());
            for dir in VCS_DIRECTORIES_TO_EXCLUDE {
                args.push(format!("--exclude-dir={}", dir));
            }
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
                args.push("-c".to_string());
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

        // `-e` keeps leading-dash patterns from being parsed as flags.
        args.push("-e".to_string());
        args.push(input.pattern.clone());

        if recursive {
            if let Some(ref glob) = input.glob {
                for pattern in glob
                    .split(|c| c == ',' || c == ' ')
                    .filter(|s| !s.is_empty())
                {
                    let pattern = pattern.trim();
                    if let Some(stripped) = pattern.strip_prefix('!') {
                        args.push(format!("--exclude={}", stripped));
                    } else {
                        args.push(format!("--include={}", pattern));
                    }
                }
            }
        }

        args.push(search_target.to_string());
        args
    }

    async fn run_grep(
        backend: &dyn agent_contracts::backend::OperationBackend,
        args: Vec<String>,
        cwd: BackendPath,
        output_mode: OutputMode,
        timeout_ms: u64,
    ) -> Result<Vec<String>, String> {
        let output = backend
            .exec()
            .exec(ExecRequest {
                command: "grep".to_string(),
                args,
                shell: None,
                cwd: Some(cwd),
                timeout_ms: Some(timeout_ms),
                env: None,
            })
            .await
            .map_err(|e| format!("Failed to execute grep via backend exec: {}", e))?;

        // grep exit codes: 0 = matches, 1 = no matches, >1 = error.
        if output.exit_code == Some(0) || output.exit_code == Some(1) {
            let stdout = String::from_utf8_lossy(output.stdout.as_slice());
            let lines = stdout
                .lines()
                .map(|line| line.replace('\r', ""))
                .filter(|line| {
                    // `grep -rc` emits `path:0` for non-matching files; ripgrep's
                    // `--count-matches` omits them. Drop zero-count lines so the
                    // downstream Count parsing matches ripgrep semantics.
                    if matches!(output_mode, OutputMode::Count) {
                        if let Some((_, count)) = line.rsplit_once(':') {
                            return count.parse::<u64>().map(|c| c != 0).unwrap_or(true);
                        }
                    }
                    true
                })
                .collect();
            return Ok(lines);
        }

        if output.timed_out {
            return Err(format!(
                "grep timed out after {}ms; narrow the search path, tighten the pattern, or pass a larger `timeout`",
                timeout_ms
            ));
        }

        Err(format!(
            "grep exited with code {:?}: {}",
            output.exit_code,
            String::from_utf8_lossy(output.stderr.as_slice())
        ))
    }

    async fn resolve_result_path(
        backend: &dyn agent_contracts::backend::OperationBackend,
        cwd: &BackendPath,
        raw_path: &str,
    ) -> Result<BackendPath, String> {
        timed(
            "grep resolve_result_path",
            DEFAULT_FS_TIMEOUT_MS,
            backend.paths().resolve_path(ResolvePathRequest {
                raw_path: raw_path.to_string(),
                base: ResolveBase::Explicit(cwd.clone()),
            }),
        )
        .await
        .map_err(|e| format!("Failed to resolve grep result path: {}", e))
    }

    async fn call_inner(
        &self,
        input: &GrepInput,
        resolved_target: &ResolvedSearchTarget,
        backend: &std::sync::Arc<dyn agent_contracts::backend::OperationBackend>,
    ) -> Result<GrepOutput, String> {
        let output_mode = input.output_mode.unwrap_or(OutputMode::FilesWithMatches);
        let head_limit = input.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
        let offset = input.offset.unwrap_or(0);
        let timeout_ms = input.timeout.unwrap_or_else(default_timeout_ms);

        let rg_args = Self::build_rg_args(input, &resolved_target.search_target);
        // Compute a single wall-clock deadline for the whole search so the
        // `rg`→`grep` fallback path cannot double the hang: the fallback only
        // spends the *remaining* budget, and is skipped entirely once `rg`
        // exhausted it (e.g. a timed-out `rg` would just time out `grep` too,
        // slower, for no benefit).
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let lines = match Self::run_rg(
            backend.as_ref(),
            rg_args,
            resolved_target.cwd.clone(),
            timeout_ms,
        )
        .await
        {
            Ok(lines) => lines,
            Err(rg_err) => {
                // `rg` is missing or errored; fall back to GNU `grep`, which is
                // present in environments (e.g. SWE-bench containers) that lack
                // ripgrep. Without this the tool fails on every call there.
                let grep_timeout = match deadline.checked_duration_since(Instant::now()) {
                    Some(remaining) if remaining.as_millis() > 0 => remaining.as_millis() as u64,
                    _ => {
                        return Err(format!(
                            "search failed (rg: {rg_err}); no time budget left for grep fallback"
                        ));
                    }
                };
                let grep_args = Self::build_grep_args(input, &resolved_target.search_target);
                Self::run_grep(
                    backend.as_ref(),
                    grep_args,
                    resolved_target.cwd.clone(),
                    output_mode,
                    grep_timeout,
                )
                .await
                .map_err(|grep_err| {
                    format!("search failed (rg: {rg_err}) (grep fallback: {grep_err})")
                })?
            }
        };

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
                // Stat all matched files concurrently via a `JoinSet` so the
                // resolve + stat syscalls run in parallel (the previous
                // sequential loop blocked for N syscalls in series on slow
                // disks/network mounts). A semaphore bounds concurrency to
                // avoid flooding the blocking pool on large match sets.
                let stat_permits = Arc::new(tokio::sync::Semaphore::new(
                    STAT_CONCURRENCY_LIMIT.min(lines.len().max(1)),
                ));
                let mut join_set = tokio::task::JoinSet::new();
                for line in lines.iter().cloned() {
                    let backend_ref = Arc::clone(backend);
                    let cwd = resolved_target.cwd.clone();
                    let permit_source = Arc::clone(&stat_permits);
                    join_set.spawn(async move {
                        let _permit = permit_source
                            .acquire()
                            .await
                            .map_err(|e| format!("stat semaphore closed: {e}"))?;
                        let resolved_path =
                            Self::resolve_result_path(backend_ref.as_ref(), &cwd, &line).await?;
                        let stat = timed(
                            "grep result stat",
                            DEFAULT_FS_TIMEOUT_MS,
                            backend_ref.files().stat(&resolved_path),
                        )
                        .await
                        .map_err(|e| format!("Failed to stat grep result file: {}", e))?;
                        Ok::<_, String>((line, stat.modified_at.unwrap_or(SystemTime::UNIX_EPOCH)))
                    });
                }

                let mut files_with_mtime: Vec<(String, SystemTime)> =
                    Vec::with_capacity(lines.len());
                while let Some(res) = join_set.join_next().await {
                    match res {
                        Ok(Ok(value)) => files_with_mtime.push(value),
                        Ok(Err(e)) => return Err(e),
                        Err(join_error) => {
                            return Err(format!("stat task panicked: {join_error}"));
                        }
                    }
                }

                // Bounded top-K via a min-heap of size `head_limit + offset`.
                // O(N log K) vs. O(N log N) for a full sort, and the
                // intermediate Vec is bounded to K. `head_limit == 0` means
                // "no limit" (heap unbounded).
                let total_matches = files_with_mtime.len();
                let want_k = if head_limit == 0 {
                    total_matches
                } else {
                    (head_limit as usize).saturating_add(offset as usize)
                };
                // Cap capacity at items we'll actually keep — `want_k` may
                // be much larger than `total_matches`.
                let heap_cap = want_k.min(total_matches).saturating_add(1).max(1);
                let mut heap: BinaryHeap<Reverse<(SystemTime, String)>> =
                    BinaryHeap::with_capacity(heap_cap);
                for (file, mtime) in files_with_mtime {
                    heap.push(Reverse((mtime, file)));
                    if want_k > 0 && heap.len() > want_k {
                        heap.pop();
                    }
                }
                let mut top_k: Vec<(SystemTime, String)> = Vec::with_capacity(heap.len());
                while let Some(Reverse((mtime, file))) = heap.pop() {
                    top_k.push((mtime, file));
                }
                // BinaryHeap::pop returns largest first → top_k is oldest-first;
                // reverse for newest-first.
                top_k.reverse();

                let skipped = top_k.len().min(offset as usize);
                let limited_files: Vec<String> = top_k
                    .into_iter()
                    .skip(skipped)
                    .map(|(_, file)| file)
                    .collect();

                let num_files = limited_files.len();
                // Truncation iff strictly more matches than kept
                // (`total_matches > want_k`). Using `num_files == head_limit`
                // would be wrong when `total_matches == head_limit + offset`
                // exactly — then `num_files == head_limit` but nothing dropped.
                let was_truncated = head_limit > 0 && total_matches > want_k;
                let applied_limit = if was_truncated {
                    Some(head_limit)
                } else {
                    None
                };

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

        match self.call_inner(&input, &resolved_target, &backend).await {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn input(pattern: &str) -> GrepInput {
        GrepInput {
            pattern: pattern.to_string(),
            ..Default::default()
        }
    }

    /// `-e` must immediately precede the pattern so it is never reparsed as a flag.
    fn pattern_is_e_guarded(args: &[String], pattern: &str) -> bool {
        args.windows(2).any(|w| w[0] == "-e" && w[1] == pattern)
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    #[test]
    fn recursive_default_mode_excludes_vcs_dirs() {
        let args = GrepExecutor::build_grep_args(&input("TODO"), ".");

        assert!(args.contains(&"-P".to_string()), "PCRE engine selected");
        assert!(args.contains(&"-r".to_string()), "recursive on directory");
        assert!(
            args.contains(&"-l".to_string()),
            "files-with-matches default"
        );
        assert!(pattern_is_e_guarded(&args, "TODO"));
        assert_eq!(
            args.last().map(String::as_str),
            Some("."),
            "target is last arg"
        );
        for dir in VCS_DIRECTORIES_TO_EXCLUDE {
            assert!(
                args.contains(&format!("--exclude-dir={}", dir)),
                "VCS dir {dir} excluded"
            );
        }
        assert!(
            !args.contains(&"-n".to_string()),
            "no line numbers outside content mode"
        );
    }

    /// A single-file target must NOT recurse: the `-r`, `--exclude-dir`, and
    /// `--include`/`--exclude` glob flags are recursive-only and would change
    /// the meaning of a one-file search if they leaked through.
    #[test]
    fn single_file_target_is_not_recursive() {
        let mut grep_input = input("TODO");
        grep_input.glob = Some("*.py".to_string());
        let args = GrepExecutor::build_grep_args(&grep_input, "foo.py");

        assert!(
            !args.contains(&"-r".to_string()),
            "no recursion on a single file"
        );
        assert!(
            !args.iter().any(|a| a.starts_with("--exclude-dir=")),
            "no VCS exclusions on a single file"
        );
        assert!(
            !args.iter().any(|a| a.starts_with("--include=")),
            "globs are recursive-only"
        );
        assert_eq!(args.last().map(String::as_str), Some("foo.py"));
    }

    #[test]
    fn content_mode_emits_line_numbers_and_context() {
        let mut grep_input = input("needle");
        grep_input.output_mode = Some(OutputMode::Content);
        grep_input.context = Some(3);
        let args = GrepExecutor::build_grep_args(&grep_input, ".");

        assert!(
            args.contains(&"-n".to_string()),
            "line numbers on by default"
        );
        assert!(
            has_pair(&args, "-C", "3"),
            "symmetric context passed through"
        );
        assert!(pattern_is_e_guarded(&args, "needle"));
    }

    #[test]
    fn count_mode_uses_dash_c() {
        let mut grep_input = input("x");
        grep_input.output_mode = Some(OutputMode::Count);
        let args = GrepExecutor::build_grep_args(&grep_input, ".");

        assert!(args.contains(&"-c".to_string()));
        assert!(!args.contains(&"-l".to_string()));
        assert!(!args.contains(&"-n".to_string()));
    }

    #[test]
    fn case_insensitive_adds_dash_i() {
        let mut grep_input = input("x");
        grep_input.case_insensitive = Some(true);
        let args = GrepExecutor::build_grep_args(&grep_input, ".");
        assert!(args.contains(&"-i".to_string()));
    }

    #[test]
    fn glob_maps_to_include_and_exclude() {
        let mut grep_input = input("x");
        grep_input.glob = Some("*.py, !*_test.py".to_string());
        let args = GrepExecutor::build_grep_args(&grep_input, ".");

        assert!(args.contains(&"--include=*.py".to_string()));
        assert!(args.contains(&"--exclude=*_test.py".to_string()));
    }

    /// A pattern beginning with `-` must not be swallowed as a grep flag.
    #[test]
    fn leading_dash_pattern_is_guarded() {
        let args = GrepExecutor::build_grep_args(&input("-n"), ".");
        assert!(pattern_is_e_guarded(&args, "-n"));
    }
}
