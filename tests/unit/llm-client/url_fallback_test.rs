use super::*;

#[test]
fn test_base_candidates_without_v1() {
    let candidates = build_base_url_candidates("http://api.test.com");
    assert_eq!(candidates.len(), 3);
    assert_eq!(candidates[0], "http://api.test.com");
    assert_eq!(candidates[1], "http://api.test.com/v1");
    assert_eq!(candidates[2], "http://api.test.com/v4");
}

#[test]
fn test_final_candidates_base_first() {
    let bases = vec![
        "http://api.test.com".to_string(),
        "http://api.test.com/v1".to_string(),
    ];
    let final_urls = build_final_candidates(&bases);

    assert_eq!(final_urls.len(), 4);
    assert_eq!(
        final_urls[0], "http://api.test.com",
        "#1 原始base（不拼接endpoint）"
    );
    assert_eq!(
        final_urls[1], "http://api.test.com/v1",
        "#2 补/v1 base（不拼接endpoint）"
    );
    assert_eq!(
        final_urls[2], "http://api.test.com/chat/completions",
        "#3 原始base + endpoint"
    );
    assert_eq!(
        final_urls[3], "http://api.test.com/v1/chat/completions",
        "#4 补/v1 base + endpoint"
    );
}

#[test]
fn test_base_candidates_with_v1() {
    let candidates = build_base_url_candidates("http://api.test.com/v1");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0], "http://api.test.com/v1");
}

#[test]
fn test_base_candidates_with_api_v1() {
    let candidates = build_base_url_candidates("http://example.com/api/v1");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0], "http://example.com/api/v1");
}

#[test]
fn test_endpoint_paths_without_v1() {
    let paths = build_endpoint_paths("http://example.com");
    assert_eq!(paths.len(), 2);
    assert_eq!(paths[0], "/chat/completions");
    assert_eq!(paths[1], "/v1/chat/completions");
}

#[test]
fn test_endpoint_paths_with_v1() {
    let paths = build_endpoint_paths("http://example.com/v1");
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "/chat/completions");
}

#[test]
fn test_no_v4_v1_combination() {
    let bases = build_base_url_candidates("http://api.test.com");
    let paths = build_endpoint_paths(&bases[2]);

    assert_eq!(bases[2], "http://api.test.com/v4");
    assert_eq!(paths.len(), 1);
    assert_eq!(paths[0], "/chat/completions");

    let final_urls = build_final_candidates(&bases);
    assert!(!final_urls.iter().any(|url| url.contains("/v4/v1/")));
}

#[test]
fn test_final_candidates_combination() {
    let bases = vec![
        "http://example.com".to_string(),
        "http://example.com/v1".to_string(),
    ];

    let final_urls = build_final_candidates(&bases);

    assert_eq!(final_urls.len(), 4);
    assert_eq!(final_urls[0], "http://example.com");
    assert_eq!(final_urls[1], "http://example.com/v1");
    assert_eq!(final_urls[2], "http://example.com/chat/completions");
    assert_eq!(final_urls[3], "http://example.com/v1/chat/completions");
}

#[test]
fn test_final_candidates_dedup() {
    let bases = vec!["http://example.com/v1".to_string()];

    let final_urls = build_final_candidates(&bases);

    assert_eq!(final_urls.len(), 2);
    assert_eq!(final_urls[0], "http://example.com/v1");
    assert_eq!(final_urls[1], "http://example.com/v1/chat/completions");
}

#[test]
fn full_chat_completions_endpoint_is_not_extended_again() {
    for endpoint in [
        "https://api.example.com/v1/chat/completions",
        "https://api.example.com/v1/chat/completions/",
        "https://api.example.com/v1/chat/completions?api-version=2026-01-01",
    ] {
        let expected = endpoint.trim_end_matches('/');
        let bases = build_base_url_candidates(endpoint);
        assert_eq!(bases, vec![expected]);

        let final_urls = build_final_candidates(&bases);
        assert_eq!(final_urls, vec![expected]);
    }
}

#[test]
fn test_endpoint_path_error_detection() {
    let http_error = LlmError::HttpError("Connection failed".to_string());
    assert!(is_endpoint_path_error(&http_error));

    let api_404 = LlmError::ApiError("HTTP 404: Not Found".to_string());
    assert!(is_endpoint_path_error(&api_404));

    let stream_error = LlmError::StreamError {
        message: "connection closed".to_string(),
    };
    assert!(is_endpoint_path_error(&stream_error));

    let parse_error = LlmError::ParseError("invalid json".to_string());
    assert!(is_endpoint_path_error(&parse_error));

    let timeout = LlmError::Timeout;
    assert!(is_endpoint_path_error(&timeout));

    let request_failed = LlmError::RequestFailed {
        message: "connection reset".to_string(),
    };
    assert!(is_endpoint_path_error(&request_failed));

    let auth_error = LlmError::AuthError {
        message: "Invalid API key".to_string(),
    };
    assert!(!is_endpoint_path_error(&auth_error));

    let cancelled = LlmError::Cancelled;
    assert!(!is_endpoint_path_error(&cancelled));
    assert!(!should_try_next_candidate(&cancelled));
}

#[test]
fn test_http_401_is_endpoint_error_to_continue_fallback() {
    // Rationale: see `is_endpoint_path_error`. The error is built through
    // the real `map_api_status_error` pipeline so this test stays in sync
    // with the wire-level message format.
    let response_body = serde_json::json!({
        "error": {
            "message": "Authentication failed: Missing API key",
            "type": "authentication_error",
            "code": 401
        }
    })
    .to_string();
    let error = crate::error::map_api_status_error(
        reqwest::StatusCode::UNAUTHORIZED,
        &response_body,
        "",
        None,
    );

    // Classified as endpoint path error → keep trying remaining candidates.
    assert!(is_http_unauthorized_error(&error));
    assert!(is_endpoint_path_error(&error));
    assert!(should_try_next_candidate(&error));

    // NOT a configuration error → does not stop the fallback early.
    assert!(!is_configuration_error(&error));
}

#[test]
fn test_http_status_code_parses_structured_prefix() {
    // Production ApiError messages are produced by `map_api_status_error`
    // as `"HTTP {status}: {body}"`. The prefix parser must extract the
    // numeric code from there and ignore HTTP-status substrings that only
    // appear in the response body (false-positive guard).
    assert_eq!(
        http_status_code("HTTP 401 Unauthorized: {\"error\":{}}"),
        Some(401)
    );
    assert_eq!(http_status_code("HTTP 404 Not Found: no body"), Some(404));
    assert_eq!(http_status_code("HTTP 400 Bad Request: bar"), Some(400));

    // A body that happens to mention another status must not leak in.
    assert_eq!(
        http_status_code("HTTP 400 Bad Request: see HTTP 401 docs"),
        Some(400)
    );
    // Non status-shaped, manually constructed ApiErrors → None (keyword
    // checks handle these instead).
    assert_eq!(http_status_code("unexpected content type: text/html"), None);
    assert_eq!(http_status_code("stream error: {\"code\":401}"), None);
    assert_eq!(http_status_code(""), None);
}

#[test]
fn test_http_404_with_invalid_keyword_is_endpoint_error() {
    // Test case from user: "HTTP 404 Not Found: Invalid URL (POST /v1)"
    // This error should be treated as endpoint path error (try other URLs),
    // NOT as configuration error (stop immediately), even though it contains "Invalid"

    let error_msg = "API error: HTTP 404 Not Found: {\"error\": \"Invalid URL (POST /v1)\"}";
    let error = LlmError::ApiError(error_msg.to_string());

    // Should be classified as endpoint path error
    assert!(is_endpoint_path_error(&error));

    // Should NOT be classified as configuration error
    assert!(!is_configuration_error(&error));

    // Should try next candidate
    assert!(should_try_next_candidate(&error));
}

#[test]
fn test_http_400_with_invalid_keyword_is_config_error() {
    // Contrast with HTTP 400: truly a configuration error

    let error_msg = "API error: HTTP 400 Bad Request: Invalid parameter";
    let error = LlmError::ApiError(error_msg.to_string());

    // Should NOT be classified as endpoint path error
    assert!(!is_endpoint_path_error(&error));

    // Should be classified as configuration error
    assert!(is_configuration_error(&error));

    // Should NOT try next candidate
    assert!(!should_try_next_candidate(&error));
}

#[test]
fn test_http_400_url_error_is_endpoint_error() {
    // Some APIs report an incomplete endpoint as HTTP 400 rather than 404.
    for message in [
        "url error",
        "url error, please check url!",
        "request failed: url error",
    ] {
        let response_body = serde_json::json!({
            "code": "InvalidParameter",
            "message": message,
        })
        .to_string();
        let error = crate::error::map_api_status_error(
            reqwest::StatusCode::BAD_REQUEST,
            &response_body,
            "",
            None,
        );

        assert!(is_endpoint_path_error(&error));
        assert!(!is_configuration_error(&error));
        assert!(should_try_next_candidate(&error));
    }
}

#[test]
fn test_error_message_generation() {
    let attempts = vec![
        UrlAttemptRecord {
            index: 0,
            url: "http://wrong.endpoint.com/chat/completions".to_string(),
            error: "HTTP 404".to_string(),
        },
        UrlAttemptRecord {
            index: 1,
            url: "http://wrong.endpoint.com/v1/chat/completions".to_string(),
            error: "Connection timeout".to_string(),
        },
    ];

    let error_msg = build_url_fallback_error_message(&attempts);

    assert!(error_msg.contains("All 2 endpoint URL candidates failed"));
    assert!(error_msg.contains("http://wrong.endpoint.com/chat/completions"));
    assert!(error_msg.contains("HTTP 404"));
    assert!(error_msg.contains("Connection timeout"));
    // The log-file-writing tail was removed; the TUI layer's
    // `remote_input` handler owns the single persisted copy now.
    assert!(!error_msg.contains("~/.xiaoo/log/error.log"));
}

#[test]
fn rate_limited_is_not_retryable() {
    // RateLimited (429/529) means quota exhausted - should fail immediately
    let error = LlmError::RateLimited {
        retry_after_ms: 5000,
        message: "Too many requests".to_string(),
    };

    assert!(!is_retryable_network_error(&error));
}

#[test]
fn test_http_502_is_retryable() {
    // HTTP 502 Bad Gateway - temporary server failure, should retry
    let error_msg = "API error: HTTP 502 Bad Gateway";
    let error = LlmError::ApiError(error_msg.to_string());

    assert!(is_retryable_network_error(&error));
}

#[test]
fn test_http_503_is_retryable() {
    // HTTP 503 Service Unavailable - temporary server failure, should retry
    let error_msg = "API error: HTTP 503 Service Unavailable";
    let error = LlmError::ApiError(error_msg.to_string());

    assert!(is_retryable_network_error(&error));
}

#[test]
fn test_http_504_is_retryable() {
    // HTTP 504 Gateway Timeout - temporary server failure, should retry
    let error_msg = "API error: HTTP 504 Gateway Timeout";
    let error = LlmError::ApiError(error_msg.to_string());

    assert!(is_retryable_network_error(&error));
}

#[test]
fn test_http_500_is_not_retryable() {
    // HTTP 500 Internal Server Error - might be persistent, not retryable here
    let error_msg = "API error: HTTP 500 Internal Server Error";
    let error = LlmError::ApiError(error_msg.to_string());

    assert!(!is_retryable_network_error(&error));
}
