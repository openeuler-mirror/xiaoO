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
                        let resolved_path = Path::new(resolved.native());
                        let parent = resolved_path
                            .parent()
                            .and_then(|value| value.to_str())
                            .ok_or_else(|| {
                                format!(
                                    "Failed to resolve parent directory for file path: {}",
                                    resolved.native()
                                )
                            })?;
                        let file_name = resolved_path
                            .file_name()
                            .and_then(|value| value.to_str())
                            .ok_or_else(|| {
                                format!(
                                    "Failed to resolve file name for path: {}",
                                    resolved.native()
                                )
                            })?;
                        Ok(ResolvedSearchTarget {
                            cwd: BackendPath::from_raw(parent.to_string()),
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
#[path = "../../../../../../../tests/unit/tool/impl/builtin/grep/executor/backend_test.rs"]
mod tests;
