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
#[path = "../../../../../tests/unit/agent-contracts/backend/capability/exec_test.rs"]
mod tests;
