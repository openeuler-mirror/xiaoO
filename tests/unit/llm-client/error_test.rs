use super::*;

#[test]
fn map_api_status_error_classifies_context_limit_failures() {
    let error = map_api_status_error(
        StatusCode::BAD_REQUEST,
        r#"{"error":{"message":"This model's maximum context length is 128000 tokens, however you requested 128500 tokens."}}"#,
        r#"{"messages":[{"content":"secret prompt"}]}"#,
        None,
    );

    assert!(matches!(error, LlmError::ContextLengthExceeded { .. }));
    assert!(!error.to_string().contains("Request body"));
    assert!(!error.to_string().contains("secret prompt"));
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
        r#"{"messages":[{"content":"secret prompt"}]}"#,
        None,
    );

    assert!(matches!(error, LlmError::ApiError(_)));
    assert!(!error.to_string().contains("Request body"));
    assert!(!error.to_string().contains("secret prompt"));
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

fn headers_with_retry_after(value: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::HeaderName::from_static("retry-after"),
        reqwest::header::HeaderValue::from_str(value).expect("valid header value"),
    );
    headers
}

#[test]
fn map_api_status_error_parses_retry_after_seconds_for_429() {
    let error = map_api_status_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"quota exceeded"}}"#,
        "{}",
        Some(&headers_with_retry_after("60")),
    );

    match error {
        LlmError::RateLimited { retry_after_ms, .. } => assert_eq!(retry_after_ms, 60_000),
        other => panic!("expected RateLimited for 429, got {:?}", other),
    }
}

#[test]
fn map_api_status_error_parses_retry_after_ms_suffix_for_429() {
    // Regression: the incident gateway sends `retry-after: 60000ms`; the
    // seconds-only parser used to reject it and fall back to 0, collapsing
    // the backoff onto the sub-second exponential floor.
    let error = map_api_status_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"quota exceeded"}}"#,
        "{}",
        Some(&headers_with_retry_after("60000ms")),
    );

    match error {
        LlmError::RateLimited { retry_after_ms, .. } => assert_eq!(retry_after_ms, 60_000),
        other => panic!("expected RateLimited for 429, got {:?}", other),
    }
}

#[test]
fn map_api_status_error_missing_429_retry_after_yields_default_window() {
    // The default is substituted at construction; a missing header must not
    // leak a zero sentinel into `retry_after_ms`.
    let error = map_api_status_error(
        StatusCode::TOO_MANY_REQUESTS,
        r#"{"error":{"message":"quota exceeded"}}"#,
        "{}",
        None,
    );

    match error {
        LlmError::RateLimited { retry_after_ms, .. } => assert_eq!(retry_after_ms, 30_000),
        other => panic!("expected RateLimited for 429, got {:?}", other),
    }
}

#[test]
fn map_api_status_error_parses_retry_after_for_529() {
    let status = StatusCode::from_u16(529).expect("529 should be a valid status code");
    let error = map_api_status_error(
        status,
        r#"{"error":{"type":"overloaded_error","message":"busy"}}"#,
        "{}",
        Some(&headers_with_retry_after("30")),
    );

    match error {
        LlmError::RateLimited { retry_after_ms, .. } => assert_eq!(retry_after_ms, 30_000),
        other => panic!("expected RateLimited for 529, got {:?}", other),
    }
}

#[test]
fn parse_retry_after_value_accepts_seconds_ms_suffix_and_fractions() {
    assert_eq!(parse_retry_after_value("60"), Some(60_000));
    assert_eq!(parse_retry_after_value(" 60 "), Some(60_000));
    assert_eq!(parse_retry_after_value("60000ms"), Some(60_000));
    assert_eq!(parse_retry_after_value("60000MS"), Some(60_000));
    assert_eq!(parse_retry_after_value("1.5"), Some(1_500));
    assert_eq!(parse_retry_after_value("soon"), None);
    assert_eq!(parse_retry_after_value("-1"), None);
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

#[path = "error_redaction_test.rs"]
mod redaction;
