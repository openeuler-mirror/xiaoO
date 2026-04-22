use std::sync::OnceLock;

pub use agent_types::LlmError;
use regex::Regex;
use reqwest::StatusCode;
use serde_json::Value;

pub(crate) fn map_reqwest_error(err: reqwest::Error) -> LlmError {
    if err.is_timeout() {
        LlmError::HttpError(format!("Request timeout: {}", err))
    } else if err.is_connect() {
        LlmError::HttpError(format!("Connection failed: {}", err))
    } else {
        LlmError::HttpError(err.to_string())
    }
}

pub(crate) fn map_serde_error(err: serde_json::Error) -> LlmError {
    LlmError::ParseError(err.to_string())
}

pub(crate) fn map_api_status_error(
    status: StatusCode,
    response_body: &str,
    request_body: &str,
    headers: Option<&reqwest::header::HeaderMap>,
) -> LlmError {
    if status.as_u16() == 529 {
        let message = if response_body.trim().is_empty() {
            "no response body".to_string()
        } else {
            response_body.to_string()
        };
        return LlmError::RateLimited {
            retry_after_ms: 5000,
            message,
        };
    }

    if status == StatusCode::TOO_MANY_REQUESTS {
        let retry_after_ms = headers
            .and_then(|h| h.get("retry-after"))
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|secs| secs * 1000)
            .unwrap_or(0);
        let message = if response_body.trim().is_empty() {
            "no response body".to_string()
        } else {
            response_body.to_string()
        };
        return LlmError::RateLimited {
            retry_after_ms,
            message,
        };
    }

    let message = format!(
        "HTTP {}: {}\nRequest body: {}",
        status, response_body, request_body
    );

    if is_context_overflow(status, response_body) {
        return LlmError::ContextLengthExceeded { message };
    }

    LlmError::ApiError(message)
}

pub(crate) fn parse_stream_error(data: &str) -> Option<LlmError> {
    let body = parse_json_object(data)?;
    if body.get("type").and_then(Value::as_str) != Some("error") {
        return None;
    }

    let response_body = serde_json::to_string(&body).ok()?;

    if error_code(&body) == Some("context_length_exceeded")
        || is_overflow_message(&extract_error_message(&body))
    {
        return Some(LlmError::ContextLengthExceeded {
            message: format!("stream error: {response_body}"),
        });
    }

    Some(LlmError::ApiError(format!("stream error: {response_body}")))
}

fn is_context_overflow(status: StatusCode, response_body: &str) -> bool {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        return true;
    }

    if let Some(body) = parse_json_object(response_body) {
        if error_code(&body) == Some("context_length_exceeded") {
            return true;
        }

        let message = extract_error_message(&body);
        if is_overflow_message(&message) {
            return true;
        }
    }

    is_overflow_message(response_body)
}

fn parse_json_object(input: &str) -> Option<Value> {
    let value: Value = serde_json::from_str(input).ok()?;
    value.is_object().then_some(value)
}

fn extract_error_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .or_else(|| body.get("error").and_then(Value::as_str))
        .or_else(|| {
            body.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
        })
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn error_code(body: &Value) -> Option<&str> {
    body.get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
}

fn is_overflow_message(message: &str) -> bool {
    if message.trim().is_empty() {
        return false;
    }

    overflow_patterns()
        .iter()
        .any(|pattern| pattern.is_match(message))
        || NO_BODY_OVERFLOW_RE
            .get_or_init(|| {
                Regex::new(r"(?i)^4(00|13)\s*(status code)?\s*\(no body\)")
                    .expect("valid no-body overflow regex")
            })
            .is_match(message)
}

fn overflow_patterns() -> &'static [Regex] {
    OVERFLOW_PATTERNS.get_or_init(|| {
        [
            r"prompt is too long",
            r"input is too long for requested model",
            r"exceeds the context window",
            r"input token count.*exceeds the maximum",
            r"maximum prompt length is \d+",
            r"reduce the length of the messages",
            r"maximum context length is \d+ tokens",
            r"exceeds the limit of \d+",
            r"exceeds the available context size",
            r"greater than the context length",
            r"context window exceeds limit",
            r"exceeded model token limit",
            r"context[_ ]length[_ ]exceeded",
            r"request entity too large",
            r"context length is only \d+ tokens",
            r"input length.*exceeds.*context length",
            r"prompt too long; exceeded (?:max )?context length",
            r"too large for model with \d+ maximum context length",
            r"model_context_window_exceeded",
        ]
        .into_iter()
        .map(|pattern| Regex::new(&format!("(?i){pattern}")).expect("valid overflow regex"))
        .collect()
    })
}

static OVERFLOW_PATTERNS: OnceLock<Vec<Regex>> = OnceLock::new();
static NO_BODY_OVERFLOW_RE: OnceLock<Regex> = OnceLock::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_api_status_error_classifies_context_limit_failures() {
        let error = map_api_status_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"message":"This model's maximum context length is 128000 tokens, however you requested 128500 tokens."}}"#,
            "{}",
            None,
        );

        assert!(matches!(error, LlmError::ContextLengthExceeded { .. }));
    }

    #[test]
    fn map_api_status_error_classifies_413_without_body_as_context_limit() {
        let error = map_api_status_error(StatusCode::PAYLOAD_TOO_LARGE, "", "{}", None);

        assert!(matches!(error, LlmError::ContextLengthExceeded { .. }));
    }

    #[test]
    fn map_api_status_error_classifies_error_code_context_length_exceeded() {
        let error = map_api_status_error(
            StatusCode::BAD_REQUEST,
            r#"{"error":{"code":"context_length_exceeded","message":"overflow"}}"#,
            "{}",
            None,
        );

        assert!(matches!(error, LlmError::ContextLengthExceeded { .. }));
    }

    #[test]
    fn map_api_status_error_keeps_non_context_failures_as_api_errors() {
        let error = map_api_status_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            r#"{"error":{"message":"provider unavailable"}}"#,
            "{}",
            None,
        );

        assert!(matches!(error, LlmError::ApiError(_)));
    }

    #[test]
    fn map_api_status_error_classifies_529_as_rate_limited_with_retry() {
        let status = StatusCode::from_u16(529).expect("529 should be a valid status code");
        let error = map_api_status_error(
            status,
            r#"{"error":{"type":"overloaded_error","message":"busy"}}"#,
            "{}",
            None,
        );

        match error {
            LlmError::RateLimited {
                retry_after_ms,
                message,
            } => {
                assert_eq!(retry_after_ms, 5000);
                assert!(message.contains("overloaded_error"));
            }
            other => panic!("expected RateLimited for 529, got {:?}", other),
        }
    }

    #[test]
    fn parse_stream_error_classifies_context_length_exceeded() {
        let error = parse_stream_error(
            r#"{"type":"error","error":{"code":"context_length_exceeded","message":"overflow"}}"#,
        );

        assert!(matches!(
            error,
            Some(LlmError::ContextLengthExceeded { .. })
        ));
    }

    #[test]
    fn parse_stream_error_keeps_other_stream_errors_as_api_errors() {
        let error = parse_stream_error(
            r#"{"type":"error","error":{"code":"invalid_prompt","message":"bad prompt"}}"#,
        );

        assert!(matches!(error, Some(LlmError::ApiError(_))));
    }
}
