use agent_types::LlmError;
use std::collections::HashSet;

pub fn build_base_url_candidates(original_base: &str) -> Vec<String> {
    let base = original_base.trim_end_matches('/');
    if is_chat_completions_endpoint(base) {
        return vec![base.to_string()];
    }
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
        if is_chat_completions_endpoint(base) {
            continue;
        }
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

fn is_chat_completions_endpoint(base: &str) -> bool {
    base.split(['?', '#'])
        .next()
        .unwrap_or(base)
        .trim_end_matches('/')
        .ends_with("/chat/completions")
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

/// Build a one-string error message summarizing why every URL candidate
/// failed. The returned string is wrapped into `LlmError::ApiError` and
/// ultimately written to `~/.xiaoo/log/error.log` by the TUI layer's
/// `remote_input` handler (`apps/endside/src/support/error_log.rs`), so this
/// function deliberately does NOT touch the log file itself to avoid
/// producing a duplicate `source=url_fallback` entry next to the
/// `source=remote_input` one.
pub fn build_url_fallback_error_message(attempts: &[UrlAttemptRecord]) -> String {
    let mut error_message = format!("All {} endpoint URL candidates failed:\n", attempts.len());
    for attempt in attempts {
        error_message.push_str(&format!(
            "  #{}: {} → {}\n",
            attempt.index + 1,
            attempt.url,
            attempt.error
        ));
    }
    error_message
}

#[derive(Debug, Clone)]
pub struct UrlAttemptRecord {
    pub index: usize,
    pub url: String,
    pub error: String,
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/url_fallback_test.rs"]
mod tests;
