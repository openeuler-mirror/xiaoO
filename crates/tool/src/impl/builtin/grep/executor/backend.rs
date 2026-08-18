use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use agent_contracts::backend::capability::exec::{ExecRequest, LineSink};
use agent_contracts::backend::capability::path::{ResolveBase, ResolvePathRequest};
use agent_contracts::backend::{BackendPath, PathKind};
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

use super::super::validation::backend as validation;
use super::constants::{
    default_timeout_ms, ABSOLUTE_HARD_CAP, DEFAULT_HEAD_LIMIT, RG_MAX_COLUMNS,
    VCS_DIRECTORIES_TO_EXCLUDE,
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

/// Resolve the caller-supplied `head_limit` into the value actually used
/// downstream (sink cap, `--max-count`, `apply_head_limit`).
///
/// Semantics:
/// - `None` (omitted) → `DEFAULT_HEAD_LIMIT` (250)
/// - `Some(0)` ("unlimited") → `ABSOLUTE_HARD_CAP` (2_000) silently. The
///   caller still gets a `truncated: true, limit: 2000` signal in the
///   output metadata if the cap is actually hit, so the silent remap is
///   transparent rather than lossy.
/// - `Some(N)` where `N > ABSOLUTE_HARD_CAP` → `ABSOLUTE_HARD_CAP`. Same
///   rationale: bound the worst case.
/// - `Some(N)` where `0 < N <= ABSOLUTE_HARD_CAP` → `N` (pass through).
///
/// Centralizing this in one place keeps `build_rg_args`, `build_grep_args`,
/// `call_inner`, and the `BoundedLineCollector` sink all in lockstep about
/// what "the user wants N lines" actually means downstream.
fn resolve_head_limit(input: &GrepInput) -> u32 {
    let raw = input.head_limit.unwrap_or(DEFAULT_HEAD_LIMIT);
    if raw == 0 || raw > ABSOLUTE_HARD_CAP {
        ABSOLUTE_HARD_CAP
    } else {
        raw
    }
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
        // Effective head_limit — resolves `0` (unlimited) and values
        // above `ABSOLUTE_HARD_CAP` down to the cap. Same helper is used
        // by `build_grep_args` and `call_inner` so all three code paths
        // agree on what "N lines" means downstream.
        let head_limit = resolve_head_limit(input);
        let offset = input.offset.unwrap_or(0);
        match output_mode {
            OutputMode::FilesWithMatches => {
                args.push("-l".to_string());
            }
            OutputMode::Count => {
                args.push("--count-matches".to_string());
            }
            OutputMode::Content => {
                // `--max-count` is a *per-file* cap in rg, not a global one.
                // It bounds the pathological single-file case (e.g. a
                // minified bundle with millions of matches) so rg doesn't
                // stream such a file's worth of output forever. The global
                // cap is enforced by `run_rg`'s streaming sink + early
                // kill, which terminates rg as soon as
                // `head_limit + offset + 1` lines have been collected
                // across all files (no stream-level `offset` skip — see
                // `BoundedLineCollector`). We pass `head_limit + offset + 1`
                // (not `head_limit + 1`) so:
                //   (a) the +1 sentinel lets `apply_head_limit` detect
                //       truncation via "more items than limit", and
                //   (b) `offset` is included so a single file with many
                //       matches still produces enough lines for
                //       `apply_head_limit` to `skip(offset).take(limit)`
                //       in one pass on the collected buffer. Without
                //       `offset` in the per-file cap, paging with
                //       `offset > 0` through a file with many matches
                //       would silently return matches from later files
                //       instead of the desired range.
                // `resolve_head_limit` already mapped `0` / above-cap
                // values to `ABSOLUTE_HARD_CAP`, so this branch always
                // passes a finite, sane value here. `saturating_add`
                // guards against u32 overflow when `offset` is large.
                if head_limit > 0 {
                    let per_file_cap = head_limit.saturating_add(offset).saturating_add(1);
                    args.push("--max-count".to_string());
                    args.push(per_file_cap.to_string());
                }

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
        head_limit: u32,
        offset: u32,
        output_mode: OutputMode,
    ) -> Result<(Vec<String>, bool), String> {
        // Stream rg's stdout line-by-line through a BoundedLineCollector.
        // The collector stops asking for more after `head_limit + 1` lines
        // (after skipping `offset`), at which point the backend kills the
        // rg process group. This bounds both memory (we never hold more
        // than `head_limit + 1` lines) and CPU (rg stops producing once
        // we've collected enough), which is the upstream throttle the old
        // fully-buffered `output.stdout` path lacked.
        //
        // For `FilesWithMatches` the cap is widened to
        // `ABSOLUTE_HARD_CAP + 1` (and stream-level `offset` skip is
        // disabled) so the downstream mtime top-K sort sees the full
        // match set — see `BoundedLineCollector::for_rg_mode`.
        //
        // The returned `bool` is `result.stopped_early` — true when the
        // collector hit its cap and asked the backend to kill rg. The
        // `FilesWithMatches` branch uses this to fix truncation
        // detection: `total_matches > want_k` alone misses the case
        // where `want_k > cap` and the collector capped (actual matches
        // exceed `want_k` but `total_matches == cap < want_k`).
        //
        // Backends without an `exec_streaming` override fall back to the
        // trait default: full-buffer `exec()` then iterate lines through
        // the sink. They lose the early-kill benefit but still bound the
        // `Vec<String>` returned here, so the downstream truncation layer
        // in `agent_loop.rs` no longer has to write oversized outputs to
        // `~/.xiaoo/truncated_tool_output/`.
        let collector = Arc::new(BoundedLineCollector::for_rg_mode(
            head_limit,
            offset,
            output_mode,
        ));
        let sink: Arc<dyn LineSink> = Arc::clone(&collector) as Arc<dyn LineSink>;

        let result = backend
            .exec()
            .exec_streaming(
                ExecRequest {
                    command: "rg".to_string(),
                    args,
                    cwd: Some(cwd),
                    timeout_ms: Some(timeout_ms),
                    ..Default::default()
                },
                sink,
            )
            .await
            .map_err(|e| format!("Failed to execute rg via backend exec_streaming: {}", e))?;

        // Exit codes: 0 = matches, 1 = no matches, >1 = error. When the
        // sink asked for early kill (`stopped_early`), exit_code is
        // typically `None` or a signal-induced value (e.g. 137 for
        // SIGKILL on Unix); we treat `stopped_early` as success since
        // *we* asked for the termination.
        if result.timed_out {
            return Err(format!(
                "rg timed out after {}ms; narrow the search path, tighten the pattern, or pass a larger `timeout`",
                timeout_ms
            ));
        }

        if !(result.exit_code == Some(0) || result.exit_code == Some(1) || result.stopped_early) {
            return Err(format!(
                "rg exited with code {:?}: {}",
                result.exit_code,
                String::from_utf8_lossy(result.stderr.as_slice())
            ));
        }

        let collector_capped = result.stopped_early;

        let lines = match Arc::try_unwrap(collector) {
            Ok(c) => c.into_lines(),
            Err(_) => panic!("BoundedLineCollector Arc must be unique after exec_streaming"),
        };
        Ok((lines, collector_capped))
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
        // Mirror `build_rg_args`: same resolution, same Content-mode-only
        // cap, same +1 sentinel + `offset` inclusion. `head_limit == 0` is
        // also remapped to `ABSOLUTE_HARD_CAP` here, so the grep fallback
        // path benefits from the same hard ceiling as the rg primary path.
        let head_limit = resolve_head_limit(input);
        let offset = input.offset.unwrap_or(0);
        match output_mode {
            OutputMode::FilesWithMatches => {
                args.push("-l".to_string());
            }
            OutputMode::Count => {
                args.push("-c".to_string());
            }
            OutputMode::Content => {
                // `-m NUM` is per-file in grep, same as rg's `--max-count`.
                // See `build_rg_args` for the rationale (bound the
                // pathological single-file case; global cap is enforced
                // downstream by `run_rg`'s streaming sink). `offset` is
                // included in the per-file cap so paging with
                // `offset > 0` through a file with many matches returns
                // the correct range rather than silently falling through to
                // later files. Note: the grep fallback path doesn't
                // currently stream — it buffers full stdout via
                // `backend.exec().exec()` — so `-m` here is the only
                // per-file bound on the fallback path.
                if head_limit > 0 {
                    let per_file_cap = head_limit.saturating_add(offset).saturating_add(1);
                    args.push("-m".to_string());
                    args.push(per_file_cap.to_string());
                }

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
        head_limit: u32,
        offset: u32,
    ) -> Result<(Vec<String>, bool), String> {
        let collector = Arc::new(BoundedLineCollector::for_rg_mode(
            head_limit,
            offset,
            output_mode,
        ));
        let sink: Arc<dyn LineSink> = Arc::clone(&collector) as Arc<dyn LineSink>;

        let result = backend
            .exec()
            .exec_streaming(
                ExecRequest {
                    command: "grep".to_string(),
                    args,
                    cwd: Some(cwd),
                    timeout_ms: Some(timeout_ms),
                    ..Default::default()
                },
                sink,
            )
            .await
            .map_err(|e| format!("Failed to execute grep via backend exec_streaming: {}", e))?;

        if result.timed_out {
            return Err(format!(
                "grep timed out after {}ms; narrow the search path, tighten the pattern, or pass a larger `timeout`",
                timeout_ms
            ));
        }

        if !(result.exit_code == Some(0) || result.exit_code == Some(1) || result.stopped_early) {
            return Err(format!(
                "grep exited with code {:?}: {}",
                result.exit_code,
                String::from_utf8_lossy(result.stderr.as_slice())
            ));
        }

        let collector_capped = result.stopped_early;
        let lines = match Arc::try_unwrap(collector) {
            Ok(c) => c.into_lines(),
            Err(_) => panic!("BoundedLineCollector Arc must be unique after exec_streaming"),
        };
        // `grep -rc` emits `path:0` for non-matching files; ripgrep's
        // `--count-matches` omits them. Drop zero-count lines in Count mode.
        let lines: Vec<String> = if matches!(output_mode, OutputMode::Count) {
            lines
                .into_iter()
                .filter(|line| {
                    line.rsplit_once(':')
                        .and_then(|(_, count)| count.parse::<u64>().ok())
                        .map(|c| c != 0)
                        .unwrap_or(true)
                })
                .collect()
        } else {
            lines
        };
        Ok((lines, collector_capped))
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
        // Use the resolved head_limit (0 → ABSOLUTE_HARD_CAP, above-cap →
        // ABSOLUTE_HARD_CAP) consistently for the streaming sink cap,
        // `apply_head_limit`, and the truncation flag. This is what
        // makes the silent remap of `head_limit: 0` transparent to the
        // user: if the cap is hit, apply_head_limit will set
        // `applied_limit = Some(ABSOLUTE_HARD_CAP)` in the output
        // metadata, signalling "you were capped at 2000; use offset
        // to page through more".
        let head_limit = resolve_head_limit(input);
        let offset = input.offset.unwrap_or(0);
        let timeout_ms = input.timeout.unwrap_or_else(default_timeout_ms);

        let rg_args = Self::build_rg_args(input, &resolved_target.search_target);
        // Compute a single wall-clock deadline for the whole search so the
        // `rg`→`grep` fallback path cannot double the hang: the fallback only
        // spends the *remaining* budget, and is skipped entirely once `rg`
        // exhausted it (e.g. a timed-out `rg` would just time out `grep` too,
        // slower, for no benefit).
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        // `collector_capped` is true when the streaming sink hit its
        // cap and asked the backend to kill rg early — i.e. there are
        // more matches than the collector's `max_keep`. Used by the
        // `FilesWithMatches` branch to fix truncation detection when
        // `want_k > max_keep` (the `total_matches > want_k` check
        // alone would miss this case). The `run_grep` fallback path
        // never caps (it buffers everything via `exec()`), so it
        // returns `false`.
        let (lines, collector_capped) = match Self::run_rg(
            backend.as_ref(),
            rg_args,
            resolved_target.cwd.clone(),
            timeout_ms,
            head_limit,
            offset,
            output_mode,
        )
        .await
        {
            Ok((lines, capped)) => (lines, capped),
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
                let (lines, collector_capped) = Self::run_grep(
                    backend.as_ref(),
                    grep_args,
                    resolved_target.cwd.clone(),
                    output_mode,
                    grep_timeout,
                    head_limit,
                    offset,
                )
                .await
                .map_err(|grep_err| {
                    format!("search failed (rg: {rg_err}) (grep fallback: {grep_err})")
                })?;
                (lines, collector_capped)
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
                let was_truncated = Self::files_with_matches_truncated(
                    head_limit,
                    total_matches,
                    want_k,
                    collector_capped,
                );
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

    /// Truncation detection for FilesWithMatches: true when
    /// `total_matches > want_k` OR the collector capped (actual
    /// matches exceed `ABSOLUTE_HARD_CAP` but `total_matches == cap`).
    /// Returns false when `head_limit == 0` (unlimited).
    fn files_with_matches_truncated(
        head_limit: u32,
        total_matches: usize,
        want_k: usize,
        collector_capped: bool,
    ) -> bool {
        head_limit > 0 && (total_matches > want_k || collector_capped)
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

/// State shared between [`BoundedLineCollector::on_line`] calls. Lives
/// behind a `Mutex` so the sink is `Send + Sync` (required by
/// `Arc<dyn LineSink>`).
#[derive(Default)]
struct BoundedCollectorState {
    kept: Vec<String>,
}

/// [`LineSink`] that bounds collected stdout to a fixed number of lines.
/// `max_keep` is `head_limit + offset + 1` for Content/Count, or
/// `ABSOLUTE_HARD_CAP + 1` for FilesWithMatches. Never pre-skips `offset`
/// at the stream level — that caused a double-skip regression.
struct BoundedLineCollector {
    max_keep: usize,
    inner: Mutex<BoundedCollectorState>,
}

impl BoundedLineCollector {
    fn new(max_keep: usize) -> Self {
        Self {
            max_keep,
            inner: Mutex::new(BoundedCollectorState::default()),
        }
    }

    /// `max_keep = head_limit + offset + 1`. The +1 sentinel lets
    /// `apply_head_limit` detect truncation; `offset` is included so
    /// downstream `skip(offset).take(limit)` works in one pass.
    /// `saturating_add` guards against u32 overflow on huge `offset`.
    fn for_head_limit(head_limit: u32, offset: u32) -> Self {
        let effective_head_limit = if head_limit == 0 {
            // Defensive: `call_inner` resolves `head_limit: 0` to
            // `ABSOLUTE_HARD_CAP` before reaching here, so this branch
            // is only hit if a future caller forgets. Mirror the remap
            // so the defensive path is consistent with the canonical one.
            ABSOLUTE_HARD_CAP
        } else {
            head_limit
        };
        // head_limit + offset + 1: the +1 sentinel lets
        // `apply_head_limit` detect truncation via "more items than
        // limit" after it skips `offset`.
        let max_keep = (effective_head_limit as usize)
            .saturating_add(offset as usize)
            .saturating_add(1);
        Self::new(max_keep)
    }

    /// FilesWithMatches: widen to `ABSOLUTE_HARD_CAP + 1` so the
    /// downstream mtime top-K sort sees the full match set. Capping at
    /// `head_limit + 1` would truncate rg's traversal-order prefix
    /// before mtime ranking.
    fn for_rg_mode(head_limit: u32, offset: u32, output_mode: OutputMode) -> Self {
        match output_mode {
            OutputMode::FilesWithMatches => {
                Self::new((ABSOLUTE_HARD_CAP as usize).saturating_add(1))
            }
            OutputMode::Content | OutputMode::Count => Self::for_head_limit(head_limit, offset),
        }
    }

    /// Take ownership of collected lines. Panics if the `Arc` is not
    /// unique (i.e. the backend's reader task is still alive) — that
    /// would be a contract violation by the backend.
    fn into_lines(self) -> Vec<String> {
        self.inner
            .into_inner()
            .expect("BoundedLineCollector mutex poisoned")
            .kept
    }
}

impl LineSink for BoundedLineCollector {
    fn on_line(&self, line: &str) -> bool {
        let mut guard = self
            .inner
            .lock()
            .expect("BoundedLineCollector mutex poisoned");
        if guard.kept.len() >= self.max_keep {
            return false;
        }
        // Strip `\r` to match the old `String::from_utf8_lossy.lines().replace('\r', "")`
        // behavior. Do NOT pre-skip `offset` here — pagination is applied
        // downstream by `apply_head_limit`.
        guard.kept.push(line.replace('\r', ""));
        true
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

    /// Default head_limit=250 → `--max-count=251` (+1 sentinel).
    #[test]
    fn rg_content_mode_adds_max_count_with_sentinel() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            has_pair(&args, "--max-count", "251"),
            "default head_limit=250 → --max-count=251 (+1 sentinel)"
        );
    }

    /// User-supplied `head_limit` is honored.
    #[test]
    fn rg_content_mode_respects_user_head_limit() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(50);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            has_pair(&args, "--max-count", "51"),
            "user head_limit=50 → --max-count=51"
        );
    }

    /// `head_limit=0` → silently capped at `ABSOLUTE_HARD_CAP=2000`.
    #[test]
    fn rg_content_mode_caps_unlimited_to_absolute_max_count() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(0);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            has_pair(&args, "--max-count", "2001"),
            "head_limit=0 → silently capped at ABSOLUTE_HARD_CAP=2000, so --max-count=2001"
        );
    }

    /// `head_limit > ABSOLUTE_HARD_CAP` → capped at `ABSOLUTE_HARD_CAP`.
    #[test]
    fn rg_content_mode_caps_above_absolute_to_max_count() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(50_000);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            has_pair(&args, "--max-count", "2001"),
            "head_limit=50000 → silently capped at ABSOLUTE_HARD_CAP=2000"
        );
    }

    /// Count mode needs accurate per-file counts; `--max-count` would
    /// corrupt them. Verify it's not applied.
    #[test]
    fn rg_count_mode_skips_max_count() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Count);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            !args.iter().any(|a| a == "--max-count"),
            "count mode must not cap per-file counts"
        );
    }

    /// FilesWithMatches is a no-op for `--max-count` (rg lists the file
    /// after 1 match regardless) — verify we don't emit it there either.
    #[test]
    fn rg_files_with_matches_skips_max_count() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::FilesWithMatches);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            !args.iter().any(|a| a == "--max-count"),
            "files-with-matches doesn't need --max-count"
        );
    }

    /// The grep fallback path gets `-m` (per-file cap) mirroring rg's
    /// `--max-count`. Same +1 sentinel semantics.
    #[test]
    fn grep_content_mode_adds_m_with_sentinel() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        let args = GrepExecutor::build_grep_args(&input, ".");
        assert!(
            has_pair(&args, "-m", "251"),
            "default head_limit=250 → -m 251 (+1 sentinel)"
        );
    }

    /// `head_limit == 0` on the grep fallback path is also silently
    /// remapped to `ABSOLUTE_HARD_CAP` — same hard cap as the rg path.
    #[test]
    fn grep_content_mode_caps_unlimited_to_absolute_m() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(0);
        let args = GrepExecutor::build_grep_args(&input, ".");
        assert!(
            has_pair(&args, "-m", "2001"),
            "head_limit=0 → silently capped at ABSOLUTE_HARD_CAP=2000, so -m 2001"
        );
    }

    /// `--max-count` includes `offset` so paging works.
    #[test]
    fn rg_content_mode_max_count_includes_offset() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(50);
        input.offset = Some(30);
        let args = GrepExecutor::build_rg_args(&input, ".");
        // head_limit=50 + offset=30 + 1 sentinel = 81
        assert!(
            has_pair(&args, "--max-count", "81"),
            "head_limit=50 + offset=30 → --max-count=81 (head_limit + offset + 1)"
        );
    }

    /// Default head_limit=250 + offset=30 → `--max-count=281`.
    #[test]
    fn rg_content_mode_max_count_default_head_limit_with_offset() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.offset = Some(30);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            has_pair(&args, "--max-count", "281"),
            "default head_limit=250 + offset=30 → --max-count=281"
        );
    }

    /// Large offset + `head_limit=0` → `2000 + 5000 + 1 = 7001`.
    #[test]
    fn rg_content_mode_max_count_offset_with_capped_head_limit() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(0);
        input.offset = Some(5_000);
        let args = GrepExecutor::build_rg_args(&input, ".");
        assert!(
            has_pair(&args, "--max-count", "7001"),
            "head_limit=0→2000 + offset=5000 → --max-count=7001"
        );
    }

    /// `offset` included in `-m` on the grep fallback path too.
    #[test]
    fn grep_content_mode_m_includes_offset() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(50);
        input.offset = Some(30);
        let args = GrepExecutor::build_grep_args(&input, ".");
        assert!(
            has_pair(&args, "-m", "81"),
            "head_limit=50 + offset=30 → -m 81 (head_limit + offset + 1)"
        );
    }

    /// Default head_limit=250 + offset=30 → `-m 281`.
    #[test]
    fn grep_content_mode_m_default_head_limit_with_offset() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.offset = Some(30);
        let args = GrepExecutor::build_grep_args(&input, ".");
        assert!(
            has_pair(&args, "-m", "281"),
            "default head_limit=250 + offset=30 → -m 281"
        );
    }

    /// `saturating_add` prevents overflow on huge `offset`.
    #[test]
    fn rg_content_mode_max_count_saturates_on_huge_offset() {
        let mut input = input("needle");
        input.output_mode = Some(OutputMode::Content);
        input.head_limit = Some(50);
        input.offset = Some(u32::MAX);
        let args = GrepExecutor::build_rg_args(&input, ".");
        // Should contain --max-count with some value (saturated, not
        // wrapped to a small number). Just verify it's present and the
        // value is >= head_limit + 1.
        let max_count = args
            .windows(2)
            .find(|w| w[0] == "--max-count")
            .map(|w| w[1].parse::<u64>().unwrap_or(0))
            .expect("--max-count must be present");
        assert!(
            max_count >= 51,
            "saturated --max-count must be >= head_limit+1=51, got {max_count}"
        );
    }

    /// Collector accepts `head_limit + 1` lines then returns `false`.
    #[test]
    fn bounded_collector_stops_at_cap() {
        // head_limit=3 → max_keep = 3 + 1 = 4
        let collector = BoundedLineCollector::for_head_limit(3, 0);
        assert!(collector.on_line("a")); // 1 kept
        assert!(collector.on_line("b")); // 2 kept
        assert!(collector.on_line("c")); // 3 kept (== head_limit)
        assert!(collector.on_line("d")); // 4 kept (the +1 sentinel)
        assert!(!collector.on_line("e")); // cap reached — ask to stop
        let lines = collector.into_lines();
        assert_eq!(lines, vec!["a", "b", "c", "d"]);
    }

    /// `resolve_head_limit`: omitted→250, 0→2000, >cap→2000, at-cap→pass.
    #[test]
    fn resolve_head_limit_handles_all_branches() {
        // Omitted (None) → DEFAULT_HEAD_LIMIT = 250
        let mut input = input("x");
        assert_eq!(resolve_head_limit(&input), 250);
        // Explicit 0 → ABSOLUTE_HARD_CAP = 2000
        input.head_limit = Some(0);
        assert_eq!(resolve_head_limit(&input), 2_000);
        // Above cap → ABSOLUTE_HARD_CAP
        input.head_limit = Some(99_999);
        assert_eq!(resolve_head_limit(&input), 2_000);
        // Exactly at cap → pass through (boundary)
        input.head_limit = Some(2_000);
        assert_eq!(resolve_head_limit(&input), 2_000);
        // Just below cap → pass through
        input.head_limit = Some(1_999);
        assert_eq!(resolve_head_limit(&input), 1_999);
        // Small finite value → pass through
        input.head_limit = Some(50);
        assert_eq!(resolve_head_limit(&input), 50);
    }

    /// Simple truncation: `total_matches > want_k`.
    #[test]
    fn files_with_matches_truncated_simple_case() {
        // head_limit=250, offset=0 → want_k=250. 300 stat'd matches.
        // 300 > 250 → truncated. (collector_capped irrelevant here.)
        assert!(GrepExecutor::files_with_matches_truncated(
            250, 300, 250, false
        ));
        // Exactly at want_k → not truncated (all fit in heap).
        assert!(!GrepExecutor::files_with_matches_truncated(
            250, 250, 250, false
        ));
        // Below want_k → not truncated.
        assert!(!GrepExecutor::files_with_matches_truncated(
            250, 100, 250, false
        ));
    }

    /// Regression: `collector_capped` must OR into truncation when
    /// `want_k > max_keep` (so `total_matches` alone misses it).
    #[test]
    fn files_with_matches_truncated_collector_capped_with_large_offset() {
        // want_k=2250 > total_matches=2001, collector capped → truncated.
        assert!(GrepExecutor::files_with_matches_truncated(
            250, 2001, 2250, true
        ));
        // Same scenario but collector did NOT cap (actual == 2001
        // exactly, stream ended naturally) → NOT truncated (the user
        // sees all 2001 files, gets 1 after offset, but that's all
        // there is — no more matches exist).
        assert!(!GrepExecutor::files_with_matches_truncated(
            250, 2001, 2250, false
        ));
        // Sanity: collector capped with small want_k → truncated via
        // the simple check (total_matches > want_k already true).
        assert!(GrepExecutor::files_with_matches_truncated(
            250, 2001, 250, true
        ));
    }

    /// `head_limit == 0` (unlimited) → never truncated.
    #[test]
    fn files_with_matches_truncated_unlimited_never_truncated() {
        assert!(!GrepExecutor::files_with_matches_truncated(
            0, 5000, 5000, true
        ));
        assert!(!GrepExecutor::files_with_matches_truncated(
            0, 2001, 2001, true
        ));
    }

    /// Collector does NOT pre-skip `offset` at the stream level —
    /// `apply_head_limit` does `skip(offset).take(limit)` downstream.
    #[test]
    fn bounded_collector_includes_offset_in_max_keep_no_stream_skip() {
        // head_limit=2, offset=3 → max_keep = 2 + 3 + 1 = 6, skip=0.
        // The collector keeps ALL 6 lines (no stream-level skip);
        // `apply_head_limit` will skip 3 and take 2 downstream.
        let collector = BoundedLineCollector::for_head_limit(2, 3);
        // First 3 lines are KEPT (not dropped) — the collector does
        // not pre-skip; downstream `apply_head_limit` handles offset.
        assert!(collector.on_line("drop1")); // kept[0]
        assert!(collector.on_line("drop2")); // kept[1]
        assert!(collector.on_line("drop3")); // kept[2]
        assert!(collector.on_line("keep1")); // kept[3]
        assert!(collector.on_line("keep2")); // kept[4]
        assert!(collector.on_line("keep3")); // kept[5] = max_keep
                                             // 7th line triggers stop — cap reached.
        assert!(!collector.on_line("keep4-overflow"));
        let lines = collector.into_lines();
        assert_eq!(
            lines,
            vec!["drop1", "drop2", "drop3", "keep1", "keep2", "keep3"]
        );
    }

    /// Regression: collector + `apply_head_limit` must not double-skip
    /// `offset`. head_limit=3, offset=4 → correct window is [L4,L5,L6].
    #[test]
    fn collector_plus_apply_head_limit_no_double_skip_for_offset() {
        let head_limit = 3u32;
        let offset = 4u32;
        let collector = BoundedLineCollector::for_head_limit(head_limit, offset);
        // rg emits 8 lines (L0..L7) — more than head_limit+offset+1=8,
        // so the collector fills to max_keep and stops early.
        for i in 0..8 {
            assert!(collector.on_line(&format!("L{i}")));
        }
        // 9th line would be rejected (max_keep reached).
        assert!(!collector.on_line("L8-overflow"));
        let lines = collector.into_lines();
        assert_eq!(
            lines.len(),
            8,
            "collector keeps head_limit+offset+1=8 lines"
        );

        // `apply_head_limit` skips offset=4, takes head_limit=3.
        let (limited, applied_limit) = GrepExecutor::apply_head_limit(lines, head_limit, offset);
        assert_eq!(
            limited,
            vec!["L4".to_string(), "L5".to_string(), "L6".to_string()],
            "double-skip bug would return empty here; correct window is [L4,L5,L6]"
        );
        // 8 items - 4 skipped = 4 remaining > head_limit=3 → truncated.
        assert_eq!(applied_limit, Some(head_limit), "truncation flag must fire");
    }

    /// `\r` is stripped from each line.
    #[test]
    fn bounded_collector_strips_carriage_returns() {
        let collector = BoundedLineCollector::for_head_limit(2, 0);
        assert!(collector.on_line("hello\r"));
        assert!(collector.on_line("world\r"));
        let lines = collector.into_lines();
        assert_eq!(lines, vec!["hello", "world"]);
    }

    /// FilesWithMatches widens cap to `ABSOLUTE_HARD_CAP + 1` so the
    /// downstream mtime top-K sort sees the full match set.
    #[test]
    fn for_rg_mode_files_with_matches_uses_wide_cap() {
        // head_limit=250 (default), offset=0, FilesWithMatches →
        // collector should accept up to ABSOLUTE_HARD_CAP + 1 = 2001
        // lines, not head_limit + 1 = 251.
        let collector = BoundedLineCollector::for_rg_mode(250, 0, OutputMode::FilesWithMatches);
        for i in 0..2_001 {
            assert!(
                collector.on_line(&format!("file-{i}")),
                "line {} should be accepted before hitting the wide cap",
                i
            );
        }
        assert!(
            !collector.on_line("file-2001-overflow"),
            "line 2002 must be rejected — ABSOLUTE_HARD_CAP+1 cap reached"
        );
        let lines = collector.into_lines();
        assert_eq!(lines.len(), 2_001);
    }

    /// FilesWithMatches doesn't pre-skip `offset` — downstream top-K
    /// + `.skip(offset)` handles pagination over the full match set.
    #[test]
    fn for_rg_mode_files_with_matches_skips_no_offset() {
        // head_limit=50, offset=100, FilesWithMatches → collector keeps
        // from line 1 (not line 101), since downstream top-K + .skip(100)
        // handles pagination.
        let collector = BoundedLineCollector::for_rg_mode(50, 100, OutputMode::FilesWithMatches);
        // First 100 lines are kept (not dropped), proving skip=0.
        for i in 0..100 {
            assert!(
                collector.on_line(&format!("file-{i}")),
                "line {} must be kept (skip=0 for FilesWithMatches)",
                i
            );
        }
        let lines = collector.into_lines();
        assert_eq!(lines.len(), 100);
        assert_eq!(lines[0], "file-0");
    }

    /// Content/Count modes: `max_keep = head_limit + offset + 1`, no stream skip.
    #[test]
    fn for_rg_mode_content_uses_tight_cap_with_offset() {
        let collector = BoundedLineCollector::for_rg_mode(3, 2, OutputMode::Content);
        assert!(collector.on_line("L0"));
        assert!(collector.on_line("L1"));
        assert!(collector.on_line("L2"));
        assert!(collector.on_line("L3"));
        assert!(collector.on_line("L4"));
        assert!(collector.on_line("L5"));
        assert!(!collector.on_line("L6-overflow"));
        let lines = collector.into_lines();
        assert_eq!(lines, vec!["L0", "L1", "L2", "L3", "L4", "L5"]);
    }
}
