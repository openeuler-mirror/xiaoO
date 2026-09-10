use crate::backend::{BackendPath, OperationError};
use async_trait::async_trait;
use std::sync::Arc;

/// Request to execute a command.
#[derive(Debug, Clone, Default)]
pub struct ExecRequest {
    pub command: String,
    pub args: Vec<String>,
    pub shell: Option<String>,
    pub cwd: Option<BackendPath>,
    pub timeout_ms: Option<u64>,
    /// Extra environment variables to inject into the process.
    pub env: Option<Vec<(String, String)>>,
    /// Additional data for a single invocation. This information
    /// originates from the pre-tool-call plugin; individual backends
    /// can parse and process the fields relevant to them.
    pub extra: Option<serde_json::Value>,
}

/// Result of command execution.
#[derive(Debug, Clone, Default)]
pub struct ExecResult {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// `true` if the line sink asked the backend to stop reading and kill
    /// the child process early. Only meaningful for streaming-capable
    /// backends that override [`OperationExec::exec_streaming`]; the
    /// default implementation never sets this flag.
    pub stopped_early: bool,
}

/// Sink for stdout lines during a streaming exec ([`OperationExec::exec_streaming`]).
///
/// Returning `false` from [`LineSink::on_line`] asks the backend to stop
/// reading stdout and kill the child process as soon as possible. This is
/// the mechanism a caller uses to bound total output: collect up to N
/// lines, then ask the backend to terminate the producer so it doesn't
/// keep emitting matches that nobody will read.
///
/// The trait is object-safe so it can be passed across the
/// `&dyn OperationExec` boundary used throughout the codebase.
pub trait LineSink: Send + Sync {
    /// Called for each line of stdout (without the trailing newline).
    /// Return `true` to keep reading, `false` to ask the backend to stop.
    fn on_line(&self, line: &str) -> bool;
}

/// Command execution capability.
#[async_trait]
pub trait OperationExec: Send + Sync {
    fn default_shell(&self) -> Option<&str> {
        None
    }
    /// Execute a command.
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError>;

    /// Stream stdout line-by-line through `sink`.
    ///
    /// The sink may return `false` from [`LineSink::on_line`] to ask the
    /// backend to stop reading and kill the child process early. This is
    /// the upstream throttle that prevents runaway `rg`/`grep` output
    /// from being fully buffered in memory or written to the
    /// `truncated_tool_output` directory — see `crates/core/src/agent_loop.rs`
    /// for how the downstream truncation layer otherwise saves every
    /// oversized tool result to disk.
    ///
    /// # Output ownership contract
    ///
    /// Callers MUST read collected state from `sink`, NOT from
    /// [`ExecResult::stdout`]. After `exec_streaming` returns, `stdout`
    /// is **always empty** (regardless of whether the sink asked to stop
    /// early or consumed everything). This contract is uniform across:
    /// - The default impl here (which clears `stdout` unconditionally
    ///   after feeding lines through the sink).
    /// - Streaming-capable backends that override this method (e.g.
    ///   `LocalExec`), which never populate `stdout` in the first place.
    ///
    /// The unconditional clear exists for two reasons:
    /// 1. Streaming-capable backends read stdout directly into the sink
    ///    and never buffer it; requiring them to *also* populate
    ///    `stdout` for the `!stopped_early` case would defeat the
    ///    streaming bound for that path.
    /// 2. Non-streaming backends falling back to this default impl
    ///    buffer stdout once in `exec()`, feed it to the sink, then
    ///    drop the buffer. Holding it for the `!stopped_early` case
    ///    would mean the full output is retained twice (in `stdout`
    ///    and in the sink) — exactly the OOM risk streaming was meant
    ///    to bound.
    ///
    /// Backends that can stream stdout (e.g. the local tokio-process
    /// backend) override this to read stdout in a line-by-line loop and
    /// kill the child on `false`. Backends that don't override it fall
    /// back to [`Self::exec`] and feed the buffered stdout through the
    /// sink without early termination — they still work, just without
    /// the streaming benefit.
    async fn exec_streaming(
        &self,
        request: ExecRequest,
        sink: Arc<dyn LineSink>,
    ) -> Result<ExecResult, OperationError> {
        exec_streaming_via_exec(self, request, sink).await
    }
}

/// Buffer-then-sink fallback for [`OperationExec::exec_streaming`].
///
/// This is the body of the trait's default `exec_streaming` impl,
/// extracted as a free function so an override can explicitly fall
/// back to the default behavior without recursing into itself.
///
/// # Why this exists
///
/// Inside an override, `OperationExec::exec_streaming(self, ...)` is
/// UFCS that resolves to `<T as OperationExec>::exec_streaming` — i.e.
/// the override itself, not the trait's default impl. Calling it from
/// the override triggers infinite recursion → stack overflow. This
/// function is the canonical "I'm an override that needs to fall back
/// to buffer-then-sink" helper: it can't be accidentally re-dispatched
/// to an override because it's a free function, not a trait method.
///
/// Used by `LocalExec::exec_streaming` when the dyn-sandbox AUTH stdin
/// channel makes line-streaming of stdout impossible — see
/// `crates/operation_backend/src/backends/local/exec.rs`.
pub async fn exec_streaming_via_exec<E: OperationExec + ?Sized>(
    backend: &E,
    request: ExecRequest,
    sink: Arc<dyn LineSink>,
) -> Result<ExecResult, OperationError> {
    let mut result = backend.exec(request).await?;
    // Scope the borrow of `result.stdout` so we can mutate it
    // (`clear()`) after the loop without violating the borrow
    // checker — `String::from_utf8_lossy` borrows `result.stdout`
    // for the lifetime of `stdout_str`.
    let stopped_early = {
        let stdout_str = String::from_utf8_lossy(&result.stdout);
        let mut stop = false;
        for line in stdout_str.lines() {
            if !sink.on_line(line) {
                stop = true;
                break;
            }
        }
        stop
    };
    // Unconditionally clear `stdout` — see the "Output ownership
    // contract" section in the trait method doc. The sink owns
    // collected state; `stdout` is not a reliable source post-return
    // for either streaming or non-streaming backends.
    result.stdout.clear();
    result.stopped_early = stopped_early;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A mock `OperationExec` that returns a fixed `stdout` from `exec`,
    /// so the default `exec_streaming` impl can be exercised without a
    /// real process. Records nothing — the tests inspect the returned
    /// `ExecResult` directly.
    struct MockExec {
        stdout: Vec<u8>,
    }

    #[async_trait]
    impl OperationExec for MockExec {
        async fn exec(&self, _request: ExecRequest) -> Result<ExecResult, OperationError> {
            Ok(ExecResult {
                stdout: self.stdout.clone(),
                stderr: Vec::new(),
                exit_code: Some(0),
                timed_out: false,
                stopped_early: false,
            })
        }
    }

    /// A `LineSink` that returns `false` after collecting `cap` lines.
    struct StopAfterN {
        cap: usize,
        seen: Mutex<usize>,
    }
    impl LineSink for StopAfterN {
        fn on_line(&self, _line: &str) -> bool {
            let mut n = self.seen.lock().unwrap();
            *n += 1;
            if *n > self.cap {
                return false;
            }
            true
        }
    }

    /// A `LineSink` that always returns `true` (consumes everything).
    struct ConsumeAll;
    impl LineSink for ConsumeAll {
        fn on_line(&self, _line: &str) -> bool {
            true
        }
    }

    /// When the sink asks to stop early, the default impl must clear
    /// `result.stdout` so the full buffered output is not retained
    /// alongside the sink's bounded subset. Without this, non-streaming
    /// backends (ConchExec, E2bExec) would hold the full stdout in
    /// memory, defeating the streaming bound.
    #[test]
    fn default_exec_streaming_clears_stdout_on_early_stop() {
        let backend = MockExec {
            stdout: b"line1\nline2\nline3\nline4\nline5\n".to_vec(),
        };
        let sink: Arc<dyn LineSink> = Arc::new(StopAfterN {
            cap: 2,
            seen: Mutex::new(0),
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime
            .block_on(OperationExec::exec_streaming(
                &backend,
                ExecRequest::default(),
                sink,
            ))
            .unwrap();

        assert!(
            result.stopped_early,
            "stopped_early must be true when the sink returned false"
        );
        assert!(
            result.stdout.is_empty(),
            "stdout must be cleared on early stop to avoid double-buffering; got {} bytes",
            result.stdout.len()
        );
        assert_eq!(result.exit_code, Some(0));
    }

    /// When the sink consumes everything (`stopped_early == false`),
    /// `result.stdout` is STILL cleared per the trait's "Output ownership
    /// contract": callers must read collected state from the sink, not
    /// from `stdout`. This guarantees streaming-capable backends and
    /// non-streaming backends behave identically from the caller's POV.
    #[test]
    fn default_exec_streaming_clears_stdout_even_when_sink_consumes_all() {
        let stdout = b"line1\nline2\nline3\n".to_vec();
        let backend = MockExec {
            stdout: stdout.clone(),
        };
        let sink: Arc<dyn LineSink> = Arc::new(ConsumeAll);

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime
            .block_on(OperationExec::exec_streaming(
                &backend,
                ExecRequest::default(),
                sink,
            ))
            .unwrap();

        assert!(!result.stopped_early);
        assert!(
            result.stdout.is_empty(),
            "stdout must be empty even when !stopped_early, per the trait contract; got {} bytes",
            result.stdout.len()
        );
    }

    /// Empty stdout + sink that returns false immediately: `stopped_early`
    /// stays false (the loop never enters), `stdout` stays empty. No
    /// panic, no spurious clear.
    #[test]
    fn default_exec_streaming_empty_stdout_no_panic() {
        let backend = MockExec { stdout: Vec::new() };
        let sink: Arc<dyn LineSink> = Arc::new(StopAfterN {
            cap: 0,
            seen: Mutex::new(0),
        });

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime
            .block_on(OperationExec::exec_streaming(
                &backend,
                ExecRequest::default(),
                sink,
            ))
            .unwrap();

        assert!(
            !result.stopped_early,
            "empty stdout → loop body never runs → stopped_early stays false"
        );
        assert!(result.stdout.is_empty());
    }
}
