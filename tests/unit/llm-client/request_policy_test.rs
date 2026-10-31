use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
#[tokio::test(start_paused = true)]
async fn hung_calls_stop_at_overall_budget() {
    let calls = AtomicU32::new(0);
    let start = tokio::time::Instant::now();
    let result: Result<(), _> = run(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending()
    })
    .await;
    // Each attempt burns the per-attempt budget, so the overall cap ends the
    // loop before MAX_NON_STREAM_RETRIES is reached.
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(start.elapsed(), REQUEST_OVERALL_BUDGET);
    assert!(matches!(
        result,
        Err(LlmError::RetryExhausted { attempts: 2, .. })
    ));
}
#[tokio::test]
async fn permanent_errors_and_cancellation_are_never_retried() {
    for error in [
        LlmError::Cancelled,
        LlmError::AuthError {
            message: "bad key".into(),
        },
        LlmError::ContextLengthExceeded {
            message: "too long".into(),
        },
        LlmError::ApiError("HTTP 400: bad request".into()),
    ] {
        let calls = AtomicU32::new(0);
        let result: Result<(), _> = run(|_| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::ready(Err(error.clone()))
        })
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
#[tokio::test(start_paused = true)]
async fn retry_after_is_capped_by_max_backoff() {
    let start = tokio::time::Instant::now();
    let result: Result<(), _> = run(|_| async {
        Err(LlmError::RateLimited {
            retry_after_ms: 600_000,
            message: "busy".into(),
        })
    })
    .await;
    assert!(matches!(
        result,
        Err(LlmError::RetryExhausted { attempts: 6, .. })
    ));
    // The huge Retry-After hint is capped, so five backoffs cost at most
    // 5 * MAX_RETRY_BACKOFF.
    assert_eq!(start.elapsed(), MAX_RETRY_BACKOFF * MAX_NON_STREAM_RETRIES);
}
