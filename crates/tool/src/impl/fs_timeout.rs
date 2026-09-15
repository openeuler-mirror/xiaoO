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
#[path = "../../../../tests/unit/tool/impl/fs_timeout_test.rs"]
mod tests;
