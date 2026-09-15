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
