//! Test-only helpers shared across the plugin adaptor test modules.

use std::future::Future;

/// Run a future on a freshly built current-thread tokio runtime with all
/// drivers enabled (IO + time + process). `tokio::process` /
/// `tokio::time::timeout` (used by `run_plugin_subprocess`) need a real
/// runtime; a noop-waker poll loop cannot drive them.
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime for test")
        .block_on(future)
}
