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
