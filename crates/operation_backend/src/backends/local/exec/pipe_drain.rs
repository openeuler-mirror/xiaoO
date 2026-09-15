use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::AsyncReadExt;

/// Maximum time to wait for the stdout/stderr reader tasks to finish after the
/// child process has exited (or been killed).
///
/// This is a safety bound only: in the common case the readers observe EOF as
/// soon as the child exits and complete immediately, adding no latency. The
/// bound exists for the case where a background process spawned by the command
/// (`foo &`) inherited the pipe and keeps its write end open, which would
/// otherwise cause the readers to block forever waiting for an EOF that never
/// arrives. One second is enough to drain anything already buffered in the
/// kernel pipe (typically 64 KiB) while still failing fast when the pipe is
/// genuinely held open by a lingering background writer.
pub(super) const DRAIN_GRACE: Duration = Duration::from_millis(1000);

/// Spawn a task that copies a child process pipe (`ChildStdout`/
/// `ChildStderr`) into a shared buffer using incremental reads.
///
/// Using incremental chunked reads into an `Arc<Mutex<Vec<u8>>>` (rather than
/// `read_to_end` into a task-owned `Vec`) means that if the task is aborted
/// while still running, any output already captured survives in the shared
/// buffer and can be returned as partial output. With `read_to_end` the buffer
/// is owned by the task and is lost on abort, which would discard the entire
/// captured stream when a lingering background writer forces us to abort.
pub(super) fn spawn_pipe_drainer<R>(
    reader: R,
    sink: Arc<Mutex<Vec<u8>>>,
) -> tokio::task::JoinHandle<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut reader = reader;
        let mut chunk = [0u8; 8192];
        loop {
            match reader.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => {
                    let mut guard = sink.lock().unwrap_or_else(|e| e.into_inner());
                    guard.extend_from_slice(&chunk[..n]);
                }
                Err(_) => break,
            }
        }
    })
}

#[cfg(all(test, unix))]
#[path = "../../../../../../tests/unit/operation_backend/backends/local/exec/pipe_drain_test.rs"]
mod pipe_drain_tests;
