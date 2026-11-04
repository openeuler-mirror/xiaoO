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
#[test]
fn rate_limited_backoff_honors_retry_after_hint() {
    // Regression: the hint used to be clamped to MAX_RETRY_BACKOFF (5s), so
    // every retry landed inside the 30-60s punishment window and rate-limited
    // lanes died as RetryExhausted.
    let backoff = retry_backoff(
        &LlmError::RateLimited {
            retry_after_ms: 60_000,
            message: "busy".into(),
        },
        0,
    );
    // Backward-only jitter: the wait is at least the hint itself.
    assert!(backoff >= Duration::from_secs(60));
    assert!(backoff <= Duration::from_secs(72));
}

#[test]
fn rate_limited_backoff_caps_huge_retry_after_hint() {
    let backoff = retry_backoff(
        &LlmError::RateLimited {
            retry_after_ms: 600_000,
            message: "busy".into(),
        },
        0,
    );
    // RATE_LIMITED_BACKOFF_CAP (120s) plus backward-only jitter:
    // [120s, 144s).
    assert!(backoff >= Duration::from_secs(120));
    assert!(backoff <= Duration::from_secs(144));
}

#[test]
fn transport_backoff_keeps_max_retry_backoff_cap() {
    let backoff = retry_backoff(&LlmError::Timeout, 10);
    // The exponential floor saturates at MAX_RETRY_BACKOFF (5s) ± 20% jitter.
    assert!(backoff >= Duration::from_secs(4));
    assert!(backoff <= Duration::from_secs(6));
}

#[test]
fn backoff_jitter_spreads_and_stays_within_twenty_percent() {
    let base = Duration::from_secs(30);
    let mut distinct = std::collections::HashSet::new();
    for _ in 0..64 {
        let backoff = jittered(base);
        assert!(backoff >= Duration::from_secs(24));
        assert!(backoff <= Duration::from_secs(36));
        distinct.insert(backoff);
    }
    // Identical samples would wake every lane in the same instant — the
    // exact thundering herd the jitter exists to break.
    assert!(distinct.len() > 1);
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

#[tokio::test(start_paused = true)]
async fn stream_rate_limited_retries_draw_their_own_allowance() {
    let calls = AtomicU32::new(0);
    let start = tokio::time::Instant::now();
    let result: Result<(), _> = run_stream(|_| {
        calls.fetch_add(1, Ordering::SeqCst);
        async {
            Err(LlmError::RateLimited {
                retry_after_ms: 60_000,
                message: "busy".into(),
            })
        }
    })
    .await;
    // One initial attempt plus the dedicated rate-limit allowance; the plain
    // transport cap of MAX_STREAM_RETRIES would have stopped at four calls.
    assert_eq!(calls.load(Ordering::SeqCst), 6);
    assert!(matches!(
        result,
        Err(LlmError::RetryExhausted { attempts: 6, .. })
    ));
    // Five backward-jittered 60s waits: [5 * 60s, 5 * 72s).
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_secs(300));
    assert!(elapsed <= Duration::from_secs(360));
}

#[tokio::test(start_paused = true)]
async fn stream_recovers_after_rate_limit_window() {
    let calls = AtomicU32::new(0);
    let start = tokio::time::Instant::now();
    let result: Result<u32, _> = run_stream(|attempt| {
        calls.fetch_add(1, Ordering::SeqCst);
        async move {
            if attempt == 0 {
                Err(LlmError::RateLimited {
                    retry_after_ms: 60_000,
                    message: "busy".into(),
                })
            } else {
                Ok(attempt)
            }
        }
    })
    .await;
    assert_eq!(result.unwrap(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // The retry never precedes the 60s window: backward-only jitter puts
    // the wait in [60s, 72s).
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_secs(60));
    assert!(elapsed <= Duration::from_secs(72));
}

#[tokio::test(start_paused = true)]
async fn non_stream_rate_limited_waits_stay_inside_overall_budget() {
    let start = tokio::time::Instant::now();
    let result: Result<(), _> = run(|_| async {
        Err(LlmError::RateLimited {
            retry_after_ms: 60_000,
            message: "busy".into(),
        })
    })
    .await;
    // With seconds-long rate-limit backoffs the overall budget — not the
    // attempt counter — is what bounds the non-streaming path.
    assert!(matches!(result, Err(LlmError::RetryExhausted { .. })));
    // Five backward-jittered 60s waits, each in [60s, 72s).
    let elapsed = start.elapsed();
    assert!(elapsed >= Duration::from_secs(300));
    assert!(elapsed < REQUEST_OVERALL_BUDGET);
}
