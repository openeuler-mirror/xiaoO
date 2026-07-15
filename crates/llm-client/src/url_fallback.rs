use agent_types::LlmError;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;

pub fn build_base_url_candidates(original_base: &str) -> Vec<String> {
    let base = original_base.trim_end_matches('/');
    let mut candidates = Vec::new();

    // B1: 用户原始配置（最高优先级）
    candidates.push(base.to_string());

    // B2: 补/v1（除原始外最高优先级）
    if !has_version_path(base) {
        candidates.push(format!("{}/v1", base));
    }

    // B3: 补/v4等其他版本（如智谱等provider）
    if !base.contains("/paas") && !has_version_path(base) {
        candidates.push(format!("{}/v4", base));
    }

    candidates
}

fn build_endpoint_paths(base: &str) -> Vec<String> {
    let base = base.trim_end_matches('/');
    let mut paths = Vec::new();

    paths.push("/chat/completions".to_string());

    if !has_version_path(base) {
        paths.push("/v1/chat/completions".to_string());
    }

    paths
}

pub fn build_final_candidates(base_candidates: &[String]) -> Vec<String> {
    let mut final_urls = Vec::new();
    let mut seen = HashSet::new();

    for base in base_candidates {
        let url = base.trim_end_matches('/').to_string();
        if !seen.contains(&url) {
            seen.insert(url.clone());
            final_urls.push(url);
        }
    }

    for base in base_candidates {
        let paths = build_endpoint_paths(base);
        for path in paths {
            let url = format!("{}{}", base.trim_end_matches('/'), path);
            let normalized_url = normalize_url_duplicates(&url);

            if !seen.contains(&normalized_url) {
                seen.insert(normalized_url.clone());
                final_urls.push(url);
            }
        }
    }

    final_urls
}

fn has_version_path(base: &str) -> bool {
    base.ends_with("/v1")
        || base.ends_with("/v4")
        || base.contains("/v1/")
        || base.contains("/v4/")
        || base.contains("/api/v1")
        || base.contains("/api/paas/v4")
}

fn normalize_url_duplicates(url: &str) -> String {
    let url = url.trim_end_matches('/');

    let patterns = ["/v1/v1", "/v4/v4"];

    for pattern in &patterns {
        if url.contains(pattern) {
            return url.replace(pattern, &pattern[..pattern.len() / 2]);
        }
    }

    url.to_string()
}

// Add each model provider's URL path error phrase as a separate entry.
const URL_PATH_ERROR_KEYWORDS: &[&str] = &[
    // Alibaba Cloud Model Studio Token Plan
    "url error",
];

/// Extracts the HTTP status code from an `ApiError` message produced by
/// [`crate::error::map_api_status_error`], which formats responses as
/// `"HTTP {status}: {body}"` (e.g. `"HTTP 401 Unauthorized: ..."`).
///
/// Returns `None` for messages that don't follow this shape — e.g.
/// manually-constructed errors such as `"unexpected content type: ..."` or
/// `"stream error: ..."`. Those are still matched by their keyword checks.
///
/// Parsing the structured prefix (instead of `contains("HTTP 401")`) avoids
/// false positives when a response *body* happens to contain an HTTP status
/// string, and survives minor wording changes in the reason phrase.
fn http_status_code(msg: &str) -> Option<u16> {
    let rest = msg.strip_prefix("HTTP ")?;
    let code = rest.split_whitespace().next()?;
    code.parse::<u16>().ok()
}

fn is_url_path_error_response(message: &str) -> bool {
    if http_status_code(message) != Some(400) {
        return false;
    }

    let Some(json_start) = message.find('{') else {
        return false;
    };
    let Ok(body) = serde_json::from_str::<serde_json::Value>(&message[json_start..]) else {
        return false;
    };
    let error_message = body
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            body.pointer("/error/message")
                .and_then(serde_json::Value::as_str)
        });

    let Some(error_message) = error_message else {
        return false;
    };
    let normalized_message = error_message.trim().to_ascii_lowercase();

    URL_PATH_ERROR_KEYWORDS
        .iter()
        .any(|keyword| normalized_message.contains(keyword))
}

/// True for an HTTP 401 (`Unauthorized`) response.
///
/// 401 is classified as an endpoint-path error (see [`is_endpoint_path_error`])
/// so the URL fallback keeps trying remaining candidates. When *every*
/// candidate returns 401, the provider uses this predicate to surface the
/// real "Unauthorized" error to the caller instead of the wrapped
/// "All N candidates failed" summary.
pub fn is_http_unauthorized_error(error: &LlmError) -> bool {
    match error {
        LlmError::ApiError(msg) => http_status_code(msg) == Some(401),
        _ => false,
    }
}

pub fn is_endpoint_path_error(error: &LlmError) -> bool {
    match error {
        LlmError::HttpError(msg) => {
            msg.contains("Connection failed")
                || msg.contains("timeout")
                || msg.contains("refused")
                || msg.contains("connect")
        }
        LlmError::ApiError(msg) => {
            // Endpoint path errors → the URL fallback tries the next candidate.
            // Some servers expose multiple chat-completion routes (e.g.
            // `/chat/completions` and `/v1/chat/completions`) and only the
            // versioned one accepts Bearer auth. An HTTP 401 from the
            // unversioned route therefore means "this route cannot authenticate
            // the request", not "the API key is wrong", so the fallback keeps
            // going. HTTP 400 with a recognized URL-error message (see
            // `is_url_path_error_response`) is treated the same way.
            //
            // When *every* candidate returns 401, the provider surfaces the
            // real "Unauthorized" error instead of the wrapped "All N
            // candidates failed" summary — see `is_http_unauthorized_error` and
            // `OpenAiFamilyProvider::complete`.
            is_http_unauthorized_error(error)
                || matches!(http_status_code(msg), Some(404) | Some(405))
                || msg.contains("Not Found")
                || msg.contains("endpoint not found")
                || msg.contains("route not found")
                || msg.contains("unexpected content type")
                || msg.contains("empty stream response")
                || is_url_path_error_response(msg)
        }
        LlmError::StreamError { .. }
        | LlmError::ParseError(_)
        | LlmError::Timeout
        | LlmError::RequestFailed { .. }
        | LlmError::IoError(_) => true,
        _ => false,
    }
}

pub fn is_configuration_error(error: &LlmError) -> bool {
    match error {
        LlmError::ApiError(msg) => {
            // Priority: endpoint path errors > configuration error markers
            //
            // Endpoint path failures can be reported in different ways:
            // - HTTP 404/405: Endpoint path errors → try other URLs
            // - HTTP 400 with a recognized URL error message → try other URLs
            // - Other HTTP 400/403 responses → stop immediately
            //
            // Configuration keywords may overlap with endpoint errors:
            // - "HTTP 404 Not Found: Invalid URL" contains "Invalid"
            // - Token Plan returns HTTP 400 with message "url error"
            //
            // Therefore, classify endpoint path errors FIRST, before checking
            // broad configuration markers such as HTTP 400/403 or "Invalid".
            if is_endpoint_path_error(error) {
                return false;
            }

            matches!(http_status_code(msg), Some(400) | Some(403))
                || msg.contains("Bad Request")
                || msg.contains("Invalid")
                || msg.contains("invalid_request_error")
        }
        LlmError::AuthError { .. }
        | LlmError::ModelNotFound { .. }
        | LlmError::RateLimited { .. }
        | LlmError::ContextLengthExceeded { .. } => true,
        _ => false,
    }
}

pub fn is_retryable_network_error(error: &LlmError) -> bool {
    match error {
        LlmError::Timeout
        | LlmError::IoError(_)
        | LlmError::StreamError { .. }
        | LlmError::ParseError(_) => true,
        LlmError::ApiError(msg) => {
            // Only retry for transient network/server errors
            // RateLimited (429/529) is quota/policy issue - should fail immediately
            matches!(http_status_code(msg), Some(502) | Some(503) | Some(504))
                || msg.contains("Bad Gateway")
                || msg.contains("Service Unavailable")
                || msg.contains("Gateway Timeout")
                || msg.contains("timeout")
                || msg.contains("network")
                || msg.contains("connection")
        }
        LlmError::RateLimited { .. } => false, // Rate limit is NOT retryable - quota exhausted
        _ => false,
    }
}

pub fn should_try_next_candidate(error: &LlmError) -> bool {
    is_endpoint_path_error(error)
}

pub fn write_url_fallback_error_log(
    original_base: &str,
    base_candidates: &[String],
    attempts: &[UrlAttemptRecord],
    final_error: &LlmError,
) -> String {
    let log_path = get_error_log_path();

    if let Some(parent) = log_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!("Failed to create error log directory: {}", e);
        }
    }

    let mut error_message = format!("All {} endpoint URL candidates failed:\n", attempts.len());

    match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(mut file) => {
            let timestamp = chrono::Local::now().to_rfc3339();

            writeln!(file, "===== {} source=url_fallback =====", timestamp).ok();
            writeln!(file, "Original API Base: {}", original_base).ok();
            writeln!(file).ok();

            writeln!(file, "Base URL Candidates Tried:").ok();
            for (idx, base) in base_candidates.iter().enumerate() {
                writeln!(file, "  B{}: {}", idx + 1, base).ok();
            }
            writeln!(file).ok();

            writeln!(file, "All Attempts Failed:").ok();
            for attempt in attempts {
                writeln!(
                    file,
                    "  #{} ({}): {}",
                    attempt.index + 1,
                    attempt.url,
                    attempt.error
                )
                .ok();
                error_message.push_str(&format!(
                    "  #{}: {} → {}\n",
                    attempt.index + 1,
                    attempt.url,
                    attempt.error
                ));
            }
            writeln!(file).ok();

            writeln!(file, "Final Error: {}", final_error).ok();
            writeln!(file).ok();

            writeln!(file, "Suggestions:").ok();
            writeln!(file, "  • Check if API base URL is correct and accessible").ok();
            let test_url = {
                let base = base_candidates.get(1).unwrap_or(&base_candidates[0]);
                if base.ends_with("/v1") || base.contains("/v1/") {
                    format!("{}{}", base.trim_end_matches('/'), "/models")
                } else {
                    format!("{}{}", base.trim_end_matches('/'), "/v1/models")
                }
            };
            writeln!(file, "  • Test endpoint manually: curl -v {}", test_url).ok();
            writeln!(file, "  • Verify API key environment variable is set").ok();
            writeln!(file, "  • Check ~/.xiaoo/log/error.log for detailed error").ok();
            writeln!(file).ok();
        }
        Err(e) => {
            tracing::error!("Failed to write error log: {}", e);
        }
    }

    error_message.push_str("Details logged to ~/.xiaoo/log/error.log");
    error_message
}

fn get_error_log_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiaoo").join("log").join("error.log"))
        .unwrap_or_else(|| PathBuf::from(".xiaoo_error.log"))
}

#[derive(Debug, Clone)]
pub struct UrlAttemptRecord {
    pub index: usize,
    pub url: String,
    pub error: String,
}

#[cfg(test)]
mod tests {
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
        let original_base = "http://wrong.endpoint.com";
        let base_candidates = vec![
            "http://wrong.endpoint.com".to_string(),
            "http://wrong.endpoint.com/v1".to_string(),
        ];
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
        let final_error = LlmError::ApiError("All candidates failed".to_string());

        let error_msg =
            write_url_fallback_error_log(original_base, &base_candidates, &attempts, &final_error);

        assert!(error_msg.contains("All 2 endpoint URL candidates failed"));
        assert!(error_msg.contains("http://wrong.endpoint.com"));
        assert!(error_msg.contains("~/.xiaoo/log/error.log"));
        assert!(error_msg.contains("http://wrong.endpoint.com/chat/completions"));
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
}
