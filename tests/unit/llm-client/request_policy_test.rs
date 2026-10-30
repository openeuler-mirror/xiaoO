use super::*;
use std::sync::atomic::{AtomicU32, Ordering};
#[tokio::test(start_paused = true)]
async fn hung_calls_stop_after_initial_attempt_and_five_retries() {
    let calls = AtomicU32::new(0);
    let start = tokio::time::Instant::now();
    let result: Result<(), _> = run(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        std::future::pending()
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert_eq!(start.elapsed(), Duration::from_secs(120));
    assert!(matches!(
        result,
        Err(LlmError::RetryExhausted { attempts: 6, .. })
    ));
}
#[tokio::test]
async fn success_on_last_retry_is_returned() {
    let result = run(|attempt| async move {
        if attempt < MAX_RETRIES {
            Err(LlmError::Timeout)
        } else {
            Ok("done")
        }
    })
    .await;
    assert_eq!(result.unwrap(), "done");
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
async fn retry_after_does_not_extend_budget() {
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
    assert_eq!(start.elapsed(), Duration::ZERO);
}
