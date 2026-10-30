//! Shared retry policy for streaming and non-streaming LLM calls.
use crate::LlmError;
use futures::{Stream, StreamExt};
use std::future::Future;
use std::time::Duration;

/// Wall-clock budget for a single non-streaming attempt. The slowest observed
/// gateway returns trivial completions in ~20-23s and real sessions carry a
/// much larger prompt, so a small fixed value times out spuriously; 180s still
/// bounds a hung upstream.
pub(crate) const REQUEST_TOTAL_BUDGET: Duration = Duration::from_secs(180);

/// Wall-clock budget across all attempts of one foreground non-streaming
/// call: two full-length attempts. Kept well below the CLI task deadline
/// (600s) so one hung call cannot consume the whole turn budget.
pub(crate) const REQUEST_OVERALL_BUDGET: Duration = Duration::from_secs(360);

/// Per-attempt budget for a background non-streaming call. Background
/// prompts can still be large, so the value stays generous enough for
/// slow-but-healthy completions, while a hung upstream cannot occupy a
/// background worker for the foreground budget's minutes.
pub(crate) const BACKGROUND_REQUEST_TOTAL_BUDGET: Duration = Duration::from_secs(120);

/// Overall budget across all attempts of one background non-streaming call:
/// two full-length attempts.
pub(crate) const BACKGROUND_REQUEST_OVERALL_BUDGET: Duration = Duration::from_secs(240);

/// TCP/TLS connect budget. Connect is fast and never the bottleneck.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Streaming: time-to-first-byte. The gateway's vLLM prefill is slow even for
/// tiny prompts, so a small value would time out spuriously. Applied twice on
/// the way to the first body byte — once to the response-head wait
/// ([`execute_stream_request`]) and once to the first chunk ([`stream_deadline`])
/// — because a stalled handshake and a stalled prefill are separate failure
/// modes.
pub(crate) const STREAM_FIRST_CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

/// Streaming: idle gap between chunks once the stream is flowing. A dead peer
/// is cut off here; a slow-but-alive generator keeps flowing.
pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) const MAX_NON_STREAM_RETRIES: u32 = 5;

/// Streaming retries. Each attempt can burn both first-byte deadlines, so
/// five retries would multiply a dead peer into roughly twelve minutes on the
/// interactive path; three keep the worst case bounded.
pub(crate) const MAX_STREAM_RETRIES: u32 = 3;

/// Backoff before a retry that carries no usable `Retry-After` hint; doubled
/// per attempt and capped at [`MAX_RETRY_BACKOFF`] so a flapping upstream is
/// not hammered.
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(200);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

fn is_retryable(error: &LlmError) -> bool {
    matches!(
        error,
        LlmError::Timeout
            | LlmError::HttpError(_)
            | LlmError::IoError(_)
            | LlmError::StreamError { .. }
            | LlmError::RateLimited { .. }
    ) || matches!(error, LlmError::ApiError(_))
        && crate::url_fallback::is_retryable_network_error(error)
}

/// Pause before retry `attempt` (0-based). A rate-limited upstream supplies a
/// `Retry-After` hint that supersedes the exponential floor, but is capped so
/// a huge hint cannot stall the caller past [`MAX_RETRY_BACKOFF`].
fn retry_backoff(error: &LlmError, attempt: u32) -> Duration {
    let exponential = RETRY_BACKOFF_BASE
        .saturating_mul(1u32 << attempt.min(5))
        .min(MAX_RETRY_BACKOFF);
    match error {
        LlmError::RateLimited { retry_after_ms, .. } => Duration::from_millis(*retry_after_ms)
            .max(exponential)
            .min(MAX_RETRY_BACKOFF),
        _ => exponential,
    }
}

/// Initial attempt plus [`MAX_NON_STREAM_RETRIES`], each attempt bounded by
/// [`REQUEST_TOTAL_BUDGET`] and the whole loop by [`REQUEST_OVERALL_BUDGET`].
pub(crate) async fn run<T, F, Fut>(call: F) -> Result<T, LlmError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    run_with_budget(
        call,
        Some(REQUEST_TOTAL_BUDGET),
        Some(REQUEST_OVERALL_BUDGET),
        MAX_NON_STREAM_RETRIES,
    )
    .await
}

/// Background variant of [`run`]: off-path callers (compaction, memory
/// summary) get a much shorter overall budget so a hung upstream cannot hold
/// a worker for the foreground cap.
pub(crate) async fn run_background<T, F, Fut>(call: F) -> Result<T, LlmError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    run_with_budget(
        call,
        Some(BACKGROUND_REQUEST_TOTAL_BUDGET),
        Some(BACKGROUND_REQUEST_OVERALL_BUDGET),
        MAX_NON_STREAM_RETRIES,
    )
    .await
}

/// Streaming variant: no total budget — a slow-but-alive stream must never be
/// cut off by a fixed deadline, so liveness is left to the provider's tiered
/// read deadlines ([`STREAM_FIRST_CHUNK_TIMEOUT`] / [`STREAM_IDLE_TIMEOUT`]).
/// Retries are capped at [`MAX_STREAM_RETRIES`].
pub(crate) async fn run_stream<T, F, Fut>(call: F) -> Result<T, LlmError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    run_with_budget(call, None, None, MAX_STREAM_RETRIES).await
}

async fn run_with_budget<T, F, Fut>(
    mut call: F,
    attempt_budget: Option<Duration>,
    overall_budget: Option<Duration>,
    max_retries: u32,
) -> Result<T, LlmError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    let overall_deadline = overall_budget.map(|budget| tokio::time::Instant::now() + budget);
    for attempt in 0..=max_retries {
        let remaining = overall_deadline
            .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()));
        if remaining.is_some_and(|remaining| remaining.is_zero()) {
            return Err(LlmError::RetryExhausted {
                attempts: attempt,
                message: "request budget exhausted before the next attempt".to_string(),
            });
        }
        // Keep each attempt inside what is left of the overall budget so one
        // slow attempt cannot overshoot the cap by a full attempt length.
        let budget = match (attempt_budget, remaining) {
            (Some(attempt), Some(remaining)) => Some(attempt.min(remaining)),
            (Some(attempt), None) => Some(attempt),
            (None, remaining) => remaining,
        };
        let error = match budget {
            Some(budget) => match tokio::time::timeout(budget, call(attempt)).await {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(error)) => error,
                Err(_) => LlmError::Timeout,
            },
            None => match call(attempt).await {
                Ok(response) => return Ok(response),
                Err(error) => error,
            },
        };
        if !is_retryable(&error) {
            return Err(error);
        }
        if attempt == max_retries {
            return Err(LlmError::RetryExhausted {
                attempts: attempt + 1,
                message: error.to_string(),
            });
        }
        let backoff = retry_backoff(&error, attempt);
        // Sleeping past the overall deadline cannot succeed: report the
        // failure now instead of stalling the caller for nothing.
        if let Some(deadline) = overall_deadline {
            if backoff >= deadline.saturating_duration_since(tokio::time::Instant::now()) {
                return Err(LlmError::RetryExhausted {
                    attempts: attempt + 1,
                    message: error.to_string(),
                });
            }
        }
        tracing::warn!(retry = attempt + 1, max_retries, error = %error,
            "LLM request failed; retrying");
        tokio::time::sleep(backoff).await;
    }
    unreachable!("the last attempt always returns")
}

/// Next stream read deadline: time-to-first-byte before any bytes arrive,
/// then the per-chunk idle budget. Any non-empty chunk (including SSE ping
/// comments) advances the counter, so a peer that only pings keeps the stream
/// alive.
pub(crate) fn stream_deadline(bytes_received: usize) -> Duration {
    if bytes_received == 0 {
        STREAM_FIRST_CHUNK_TIMEOUT
    } else {
        STREAM_IDLE_TIMEOUT
    }
}

/// Sends a prepared streaming request and awaits the response head under
/// [`STREAM_FIRST_CHUNK_TIMEOUT`]. Without this, a peer that completes the
/// handshake but never answers would pin the attempt forever: `run_stream`
/// has no total deadline, and [`stream_deadline`] only starts once the head
/// (and thus the body stream) exists.
pub(crate) async fn execute_stream_request<F>(
    client: &reqwest::Client,
    request: reqwest::Request,
    map_error: F,
) -> Result<reqwest::Response, LlmError>
where
    F: FnOnce(reqwest::Error) -> LlmError,
{
    match tokio::time::timeout(STREAM_FIRST_CHUNK_TIMEOUT, client.execute(request)).await {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => Err(map_error(error)),
        Err(_) => Err(LlmError::Timeout),
    }
}

/// Awaits the next body chunk under [`stream_deadline`], returning the caller's
/// own transport error untouched so each provider can map it with its own
/// context. Fires [`LlmError::Timeout`] when the deadline elapses.
pub(crate) async fn next_stream_chunk<S, T, E>(
    stream: &mut S,
    bytes_received: usize,
) -> Result<Option<Result<T, E>>, LlmError>
where
    S: Stream<Item = Result<T, E>> + Unpin,
{
    match tokio::time::timeout(stream_deadline(bytes_received), stream.next()).await {
        Ok(item) => Ok(item),
        Err(_) => Err(LlmError::Timeout),
    }
}

/// Reads an error / protocol-mismatch body under the first-chunk budget.
/// Read failures degrade to an empty body, while a stalled body becomes a
/// retryable timeout instead of an unbounded hang on the streaming path
/// (which has no overall deadline).
pub(crate) async fn read_error_body_with_deadline(
    response: reqwest::Response,
) -> Result<String, LlmError> {
    match tokio::time::timeout(STREAM_FIRST_CHUNK_TIMEOUT, response.text()).await {
        Ok(Ok(body)) => Ok(body),
        Ok(Err(_)) => Ok(String::new()),
        Err(_) => Err(LlmError::Timeout),
    }
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/request_policy_test.rs"]
mod tests;
