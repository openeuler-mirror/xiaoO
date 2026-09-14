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
mod pipe_drain_tests {
    use super::super::command_spec::test_workspace_root;
    use crate::backends::local::factory::local_backend;
    use agent_contracts::backend::capability::exec::LineSink;
    use agent_contracts::backend::{capability::exec::ExecRequest, BackendPath};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    // Regression test for the stdout-pipe deadlock: a background process spawned
    // by the script (`sleep 3 &`) inherits bash's piped stdout and keeps its
    // write end open for longer than the exec timeout. The old implementation
    // only wrapped `child.wait()` in the timeout; bash exits almost instantly,
    // then the subsequent unbounded `stdout_task.await` blocked until the
    // background `sleep` finally exited (~3s) and released the pipe. With the
    // fix, the post-exit drain is bounded by `DRAIN_GRACE` (1s), so exec returns
    // promptly with the partial output captured before the drain gave up.
    #[test]
    fn exec_returns_promptly_when_background_process_holds_stdout_pipe() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = test_workspace_root("xiaoo-pipe-", "pipe");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let command = "echo main-output; sleep 3 &";
        let start = Instant::now();
        let result = runtime.block_on(async {
            backend
                .exec()
                .exec(ExecRequest {
                    command: command.to_string(),
                    args: vec![],
                    shell: Some("bash".to_string()),
                    cwd: Some(BackendPath::from_raw(
                        workspace.to_string_lossy().into_owned(),
                    )),
                    timeout_ms: Some(2_000),
                    ..Default::default()
                })
                .await
                .unwrap()
        });
        let elapsed = start.elapsed();

        assert_eq!(result.exit_code, Some(0));
        assert!(!result.timed_out, "should not hit the overall timeout");
        let out = String::from_utf8_lossy(result.stdout.as_slice());
        assert!(out.contains("main-output"), "captured stdout was: {out:?}");
        // The background `sleep 3` keeps the pipe open for 3s. The fix must
        // return within the ~1s drain grace, well before that — the old impl
        // returned only after ~3s.
        assert!(
            elapsed < Duration::from_secs(2),
            "exec took {elapsed:?}, expected to return within the drain grace (~1s)"
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    // When the background writer releases the pipe on its own *before* the
    // drain grace elapses, the reader observes EOF normally and we capture the
    // full stream including the background writer's output.
    #[test]
    fn exec_captures_background_output_when_writer_exits_within_grace() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = test_workspace_root("xiaoo-pipe-", "bg");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        // The background subshell writes three lines over ~0.3s then exits,
        // releasing the inherited stdout pipe. All of it should be captured.
        let command = "echo main; ( for i in 1 2 3; do echo bg-$i; sleep 0.1; done ) & echo done";
        let result = runtime.block_on(async {
            backend
                .exec()
                .exec(ExecRequest {
                    command: command.to_string(),
                    args: vec![],
                    shell: Some("bash".to_string()),
                    cwd: Some(BackendPath::from_raw(
                        workspace.to_string_lossy().into_owned(),
                    )),
                    timeout_ms: Some(5_000),
                    ..Default::default()
                })
                .await
                .unwrap()
        });

        assert_eq!(result.exit_code, Some(0));
        let out = String::from_utf8_lossy(result.stdout.as_slice());
        assert!(out.contains("main"), "stdout was: {out:?}");
        assert!(out.contains("done"), "stdout was: {out:?}");
        assert!(out.contains("bg-1"), "stdout was: {out:?}");
        assert!(out.contains("bg-3"), "stdout was: {out:?}");

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    // Streaming exec + line sink that returns `false` after N lines must
    // (a) kill the child process group, (b) set `stopped_early = true`,
    // (c) not buffer the remaining N+1..∞ lines into memory. This is the
    // upstream throttle that bounds runaway `rg`/`grep` output before it
    // ever reaches the downstream truncation layer in `agent_loop.rs`.
    //
    // We use `seq 1 1_000_000` as the producer (one million lines, takes
    // seconds to fully emit) and a sink that stops after 5 lines. The
    // streaming exec must return well before the producer finishes
    // (otherwise this test would take seconds).
    struct FiveLineCollector {
        kept: Mutex<Vec<String>>,
    }
    impl LineSink for FiveLineCollector {
        fn on_line(&self, line: &str) -> bool {
            let mut guard = self.kept.lock().unwrap();
            if guard.len() >= 5 {
                return false;
            }
            guard.push(line.to_string());
            true
        }
    }

    #[test]
    fn exec_streaming_kills_child_when_sink_returns_false() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = test_workspace_root("xiaoo-stream-", "kill");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let collector = Arc::new(FiveLineCollector {
            kept: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn LineSink> = Arc::clone(&collector) as Arc<dyn LineSink>;

        let start = Instant::now();
        let result = runtime.block_on(async {
            backend
                .exec()
                .exec_streaming(
                    ExecRequest {
                        // `seq 1 1000000` emits a million lines over a few
                        // seconds. If streaming + early kill works, we return
                        // immediately after 5 lines.
                        command: "seq 1 1000000".to_string(),
                        args: vec![],
                        shell: Some("bash".to_string()),
                        cwd: Some(BackendPath::from_raw(
                            workspace.to_string_lossy().into_owned(),
                        )),
                        timeout_ms: Some(30_000), // generous upper bound; we expect to return way before
                        ..Default::default()
                    },
                    sink,
                )
                .await
                .unwrap()
        });
        let elapsed = start.elapsed();

        // Sink asked for early termination — flag must be set.
        assert!(
            result.stopped_early,
            "stopped_early must be true when the sink returned false; got {result:?}"
        );

        // We must NOT have hit the overall timeout — that's the whole
        // point of streaming + early kill.
        assert!(
            !result.timed_out,
            "must not time out; expected to kill child after 5 lines"
        );

        // Bounded wall-clock: a million lines take seconds to emit
        // fully. The early-kill path should return in well under a
        // second (5 lines + process-group teardown ~300ms grace).
        assert!(
            elapsed < Duration::from_secs(5),
            "exec_streaming took {elapsed:?}, expected to return within seconds after early kill"
        );

        // Sink collected exactly 5 lines (the cap), no more — i.e. the
        // remaining 999,995 lines were never read into memory.
        let guard = collector.kept.lock().unwrap();
        assert_eq!(
            guard.len(),
            5,
            "collector must hold exactly 5 lines, got {}",
            guard.len()
        );
        assert_eq!(guard[0], "1");
        assert_eq!(guard[4], "5");

        // stdout in the result is empty — the sink owns collected state,
        // streaming bypassed the buffering.
        assert!(
            result.stdout.is_empty(),
            "streaming result.stdout must be empty (sink owns state); got {} bytes",
            result.stdout.len()
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    // Streaming exec with a sink that never returns `false` (consumer
    // wants everything) must behave like the non-streaming `exec` —
    // exit code is the child's natural exit, `stopped_early = false`,
    // and the sink sees every line in order.
    struct AllLinesCollector {
        kept: Mutex<Vec<String>>,
    }
    impl LineSink for AllLinesCollector {
        fn on_line(&self, line: &str) -> bool {
            self.kept.lock().unwrap().push(line.to_string());
            true
        }
    }

    #[test]
    fn exec_streaming_drains_to_eof_when_sink_never_stops() {
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = test_workspace_root("xiaoo-stream-", "eof");
        let workspace = root.join("workspace");

        let backend =
            local_backend(workspace.clone(), None, None, Some("bash".to_string())).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let collector = Arc::new(AllLinesCollector {
            kept: Mutex::new(Vec::new()),
        });
        let sink: Arc<dyn LineSink> = Arc::clone(&collector) as Arc<dyn LineSink>;

        let result = runtime.block_on(async {
            backend
                .exec()
                .exec_streaming(
                    ExecRequest {
                        command: "printf 'a\\nb\\nc\\n'".to_string(),
                        args: vec![],
                        shell: Some("bash".to_string()),
                        cwd: Some(BackendPath::from_raw(
                            workspace.to_string_lossy().into_owned(),
                        )),
                        timeout_ms: Some(5_000),
                        ..Default::default()
                    },
                    sink,
                )
                .await
                .unwrap()
        });

        // Child exited naturally → exit_code 0, no early kill.
        assert_eq!(result.exit_code, Some(0));
        assert!(!result.stopped_early);
        assert!(!result.timed_out);

        // Sink observed all 3 lines in order.
        let guard = collector.kept.lock().unwrap();
        assert_eq!(*guard, vec!["a", "b", "c"]);

        let _ = std::fs::remove_dir_all(root.as_path());
    }
}
