//! Wall-clock timeout wrappers for operation-backend calls.
//!
//! The `OperationBackend` capability traits (`files`, `search`, ...) do not
//! impose their own deadlines, so a hung mount, a dead remote sandbox, or a
//! pathological directory tree can park a tool call forever. These helpers
//! apply a uniform safety net at the tool layer without touching the trait
//! contract shared by every backend implementation.

use std::time::Duration;
use tokio::time::timeout;

/// Default wall-clock budget for a single filesystem/search backend call.
///
/// Local disks finish in milliseconds; HTTP-backed sandboxes (E2B/Conch) take
/// low single-digit seconds for typical payloads. 30s is a generous safety net
/// that still bounds the worst case so a single stuck call cannot hang an agent
/// loop indefinitely.
pub const DEFAULT_FS_TIMEOUT_MS: u64 = 30_000;

/// Run `fut` with a wall-clock deadline of `timeout_ms`.
///
/// On timeout returns `Err("{label} timed out after {ms}ms")`; on inner failure
/// forwards the inner error's `Display` rendering. Callers are expected to wrap
/// the `String` into whatever tool-specific error type they use.
pub async fn timed<F, T, E>(label: &str, timeout_ms: u64, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    match timeout(Duration::from_millis(timeout_ms), fut).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Err(format!("{} timed out after {}ms", label, timeout_ms)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn fast_future_returns_value_unchanged() {
        let fut = async { Ok::<u32, &str>(42) };
        let result = timed("op", 1000, fut).await;
        assert_eq!(result, Ok(42));
    }

    #[tokio::test]
    async fn inner_error_display_is_forwarded() {
        let fut = async { Err::<u32, String>("boom".to_string()) };
        let result = timed("op", 1000, fut).await;
        assert_eq!(result.unwrap_err(), "boom");
    }

    #[tokio::test]
    async fn slow_future_times_out_with_label_and_duration() {
        // 5s sleep guarded by a 50ms budget — must hit the timeout branch.
        let fut = async {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok::<(), &str>(())
        };
        let result = timed("slow op", 50, fut).await;
        let err = result.unwrap_err();
        assert!(err.contains("slow op"), "missing label: {err}");
        assert!(err.contains("50ms"), "missing duration: {err}");
        assert!(err.contains("timed out"), "missing keyword: {err}");
    }

    #[tokio::test]
    async fn inner_error_takes_precedence_over_implicit_deadline() {
        // Future resolves with an error well before the deadline: the error
        // path, not the timeout path, must be reported.
        let fut = async { Err::<u32, &str>("fail") };
        let result = timed("op", 10_000, fut).await;
        assert_eq!(result.unwrap_err(), "fail");
    }
}
