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

#[tokio::test(start_paused = true)]
async fn slow_headers_and_prefill_use_independent_io_budgets() {
    let calls = AtomicU32::new(0);
    let started = tokio::time::Instant::now();
    let result = run_stream(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        async {
            tokio::time::sleep(Duration::from_secs(50)).await;
            let mut body = Box::pin(futures::stream::once(async {
                tokio::time::sleep(Duration::from_secs(50)).await;
                Ok::<_, LlmError>(vec![1u8])
            }));
            next_stream_chunk(&mut body, 0).await?.unwrap()
        }
    })
    .await;
    assert_eq!(result.unwrap(), vec![1]);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(started.elapsed(), Duration::from_secs(100));
}

#[tokio::test(start_paused = true)]
async fn raw_ping_chunks_keep_slow_body_reads_alive() {
    let started = tokio::time::Instant::now();
    let mut body = Box::pin(async_stream::stream! {
        tokio::time::sleep(Duration::from_secs(23)).await;
        yield Ok::<_, LlmError>(b": ping\n\n".to_vec());
        for _ in 0..3 {
            tokio::time::sleep(Duration::from_secs(25)).await;
            yield Ok(b": ping\n\n".to_vec());
        }
    });
    let mut received = 0;
    while let Some(chunk) = next_stream_chunk(&mut body, received).await.unwrap() {
        received += chunk.unwrap().len();
    }
    assert!(received > 0);
    assert_eq!(started.elapsed(), Duration::from_secs(98));
}

#[tokio::test(start_paused = true)]
async fn stalled_first_body_read_times_out_at_sixty_seconds() {
    let started = tokio::time::Instant::now();
    let mut body = futures::stream::pending::<Result<Vec<u8>, LlmError>>();
    assert!(matches!(
        next_stream_chunk(&mut body, 0).await,
        Err(LlmError::Timeout)
    ));
    assert_eq!(started.elapsed(), Duration::from_secs(60));
}

#[tokio::test(start_paused = true)]
async fn stalled_flowing_body_times_out_at_thirty_seconds() {
    let started = tokio::time::Instant::now();
    let mut body = futures::stream::pending::<Result<Vec<u8>, LlmError>>();
    assert!(matches!(
        next_stream_chunk(&mut body, 10).await,
        Err(LlmError::Timeout)
    ));
    assert_eq!(started.elapsed(), Duration::from_secs(30));
}

#[tokio::test(start_paused = true)]
async fn stalled_transport_retries_at_most_three_times() {
    let calls = AtomicU32::new(0);
    let result: Result<(), _> = run_stream(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        async {
            let mut body = futures::stream::pending::<Result<Vec<u8>, LlmError>>();
            next_stream_chunk(&mut body, 0).await?;
            Ok(())
        }
    })
    .await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    assert!(matches!(
        result,
        Err(LlmError::RetryExhausted { attempts: 4, .. })
    ));
}
