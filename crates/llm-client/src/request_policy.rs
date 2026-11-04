//! Shared retry policy for streaming and non-streaming LLM calls.
use crate::LlmError;
use futures::{Stream, StreamExt};
use rand::Rng;
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
/// tiny prompts, so a small value would time out spuriously. Give the response
/// head (connection/TLS included) and the first body chunk separate 60s budgets.
pub(crate) const STREAM_FIRST_CHUNK_TIMEOUT: Duration = Duration::from_secs(60);

/// Streaming: idle gap after body data starts flowing. A slow-but-alive
/// generator keeps flowing; only a gap of 30s without data times out.
pub(crate) const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const MAX_NON_STREAM_RETRIES: u32 = 5;

/// Each stalled streaming attempt may use two first-response budgets; keep
/// retries at three so repeated prefill does not amplify a slow gateway.
pub(crate) const MAX_STREAM_RETRIES: u32 = 3;

/// Backoff before a retry that carries no usable `Retry-After` hint; doubled
/// per attempt and capped at [`MAX_RETRY_BACKOFF`] so a flapping upstream is
/// not hammered. Rate-limited responses do NOT use this cap — see
/// [`RATE_LIMITED_BACKOFF_CAP`].
const RETRY_BACKOFF_BASE: Duration = Duration::from_millis(200);
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// Cap for a server-provided `Retry-After` hint. Rate-limit windows run far
/// longer than the 5s transport-error cap, so `RateLimited` gets its own
/// ceiling; the cap still protects callers from a misbehaving gateway that
/// hints at eternity. Hint parsing clamps to this same bound (see
/// `crate::error::parse_retry_after_value`) so user-visible errors and hook
/// payloads never advertise a wait the policy would not honor.
pub(crate) const RATE_LIMITED_BACKOFF_CAP: Duration = Duration::from_secs(120);

/// Streaming-only retry allowance for `RateLimited`. Waiting out the quota
/// window usually succeeds on the next attempt, so streams (which have no
/// overall budget) may wait more often for a rate limit than for transport
/// errors — but finitely often, so a permanently exhausted quota cannot
/// stall a stream indefinitely.
const MAX_STREAM_RATE_LIMIT_RETRIES: u32 = 5;

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

/// ±20% jitter for transport-error backoffs: lanes that fail together would
/// otherwise wake in lockstep and re-crush a still-recovering upstream
/// (thundering herd), so wake-up moments are spread around the backoff.
fn jittered(base: Duration) -> Duration {
    let factor = rand::thread_rng().gen_range(0.8f64..1.2f64);
    base.mul_f64(factor)
}

/// Backward-only jitter for rate-limit waits: waking before the server's
/// `Retry-After` hint lands inside the quota window and burns one of the few
/// [`MAX_STREAM_RATE_LIMIT_RETRIES`] allowances, so wake-ups spread into
/// [base, 1.2·base) — still staggered enough to break the same herd.
fn rate_limit_jittered(base: Duration) -> Duration {
    let factor = rand::thread_rng().gen_range(1.0f64..1.2f64);
    base.mul_f64(factor)
}

/// Pause before retry `attempt` (0-based). A rate-limited upstream supplies a
/// `Retry-After` hint that supersedes the exponential floor; the hint is
/// capped at [`RATE_LIMITED_BACKOFF_CAP`] rather than [`MAX_RETRY_BACKOFF`]
/// because quota windows (30-60s) far outlast the transport-error ceiling.
/// The error layer substitutes its quota-window default for a missing hint,
/// so a zero here only arises from a hand-built error and falls through to
/// the exponential floor.
fn retry_backoff(error: &LlmError, attempt: u32) -> Duration {
    let exponential = RETRY_BACKOFF_BASE
        .saturating_mul(1u32 << attempt.min(5))
        .min(MAX_RETRY_BACKOFF);
    match error {
        LlmError::RateLimited { retry_after_ms, .. } => rate_limit_jittered(
            Duration::from_millis(*retry_after_ms)
                .max(exponential)
                .min(RATE_LIMITED_BACKOFF_CAP),
        ),
        _ => jittered(exponential),
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

/// Streaming has no wrapper-level duration cap. Providers enforce response
/// and idle deadlines at their actual I/O boundary, including across spawned
/// tasks and raw provider calls. Parsed chunks alone cannot measure liveness:
/// transport heartbeats and partial frames may not produce callback events.
/// Retries are capped at [`MAX_STREAM_RETRIES`]; rate-limited attempts draw
/// from the larger [`MAX_STREAM_RATE_LIMIT_RETRIES`] allowance because
/// waiting out the quota window usually succeeds on the next attempt.
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
    let mut attempt: u32 = 0;
    loop {
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
        // The budget also covers any admission-gate queueing inside the
        // provider: the LLM gate sits under `LlmProviderWrapper`'s retry loop,
        // so a saturated gate can surface as `Timeout` even though the gateway
        // never received the request. The gate logs throttling separately;
        // see `crate::rate_limit`.
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
        // Streaming carries no overall budget, so RateLimited gets its own
        // finite allowance larger than the transport-retry cap (see
        // [`MAX_STREAM_RATE_LIMIT_RETRIES`]). Non-streaming keeps the plain
        // cap: its overall budget already bounds how long rate-limit waits
        // may accumulate.
        let effective_max_retries =
            if overall_budget.is_none() && matches!(error, LlmError::RateLimited { .. }) {
                max_retries.max(MAX_STREAM_RATE_LIMIT_RETRIES)
            } else {
                max_retries
            };
        if attempt >= effective_max_retries {
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
        tracing::warn!(retry = attempt + 1, max_retries = effective_max_retries,
            backoff_ms = backoff.as_millis() as u64, error = %error,
            "LLM request failed; retrying");
        tokio::time::sleep(backoff).await;
        attempt += 1;
    }
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
/// [`STREAM_FIRST_CHUNK_TIMEOUT`], also protecting raw provider calls that
/// bypass the wrapper. The first body read starts its own budget in
/// [`next_stream_chunk`]; the two phases do not share one deadline.
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
        Err(_) => {
            tracing::warn!(
                phase = "response_head",
                timeout_secs = STREAM_FIRST_CHUNK_TIMEOUT.as_secs(),
                "LLM streaming response timed out"
            );
            Err(LlmError::Timeout)
        }
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
    let deadline = stream_deadline(bytes_received);
    match tokio::time::timeout(deadline, stream.next()).await {
        Ok(item) => Ok(item),
        Err(_) => {
            let phase = if bytes_received == 0 {
                "first_body"
            } else {
                "stream_idle"
            };
            tracing::warn!(
                phase,
                bytes_received,
                timeout_secs = deadline.as_secs(),
                "LLM streaming read timed out"
            );
            Err(LlmError::Timeout)
        }
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
