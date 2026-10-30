//! Shared retry budget for streaming and non-streaming LLM calls.
use crate::LlmError;
use std::future::Future;
use std::time::Duration;

pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const MAX_RETRIES: u32 = 5;

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

/// Initial attempt plus five retries, each bounded independently.
/// Backoff and Retry-After sleeps must not extend this budget.
pub(crate) async fn run<T, F, Fut>(mut call: F) -> Result<T, LlmError>
where
    F: FnMut(u32) -> Fut,
    Fut: Future<Output = Result<T, LlmError>>,
{
    for attempt in 0..=MAX_RETRIES {
        let error = match tokio::time::timeout(REQUEST_TIMEOUT, call(attempt)).await {
            Ok(Ok(response)) => return Ok(response),
            Ok(Err(error)) => error,
            Err(_) => LlmError::Timeout,
        };
        if !is_retryable(&error) {
            return Err(error);
        }
        if attempt == MAX_RETRIES {
            return Err(LlmError::RetryExhausted {
                attempts: attempt + 1,
                message: error.to_string(),
            });
        }
        tracing::warn!(retry = attempt + 1, max_retries = MAX_RETRIES,
            timeout_secs = REQUEST_TIMEOUT.as_secs(), error = %error,
            "LLM request failed; retrying");
    }
    unreachable!("the last attempt always returns")
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/request_policy_test.rs"]
mod tests;
