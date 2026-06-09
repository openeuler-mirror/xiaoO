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

    let patterns = [
        "/v1/v1",
        "/v4/v4",
    ];

    for pattern in &patterns {
        if url.contains(pattern) {
            return url.replace(pattern, &pattern[..pattern.len() / 2]);
        }
    }

    url.to_string()
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
            msg.contains("HTTP 404")
                || msg.contains("HTTP 405")
                || msg.contains("Not Found")
                || msg.contains("endpoint not found")
                || msg.contains("route not found")
                || msg.contains("unexpected content type")
                || msg.contains("empty stream response")
        }
        LlmError::StreamError { .. }
        | LlmError::ParseError(_)
        | LlmError::Timeout
        | LlmError::RequestFailed { .. }
        | LlmError::IoError(_) => true,
        _ => false,
    }
}

pub fn should_try_next_candidate(error: &LlmError) -> bool {
    is_endpoint_path_error(error)
        && !matches!(
            error,
            LlmError::AuthError { .. }
                | LlmError::ModelNotFound { .. }
                | LlmError::RateLimited { .. }
                | LlmError::ContextLengthExceeded { .. }
                | LlmError::Cancelled
        )
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

    let mut error_message = format!(
        "All {} endpoint URL candidates failed:\n",
        attempts.len()
    );

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
            writeln!(
                file,
                "  • Check if API base URL is correct and accessible"
            )
            .ok();
            writeln!(
                file,
                "  • Test endpoint manually: curl -v {}/v1/models",
                base_candidates.get(1).unwrap_or(&base_candidates[0])
            )
            .ok();
            writeln!(
                file,
                "  • Verify API key environment variable is set"
            )
            .ok();
            writeln!(
                file,
                "  • Check ~/.xiaoo/log/error.log for detailed error"
            )
            .ok();
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
        assert_eq!(final_urls[0], "http://api.test.com", "#1 原始base（不拼接endpoint）");
        assert_eq!(final_urls[1], "http://api.test.com/v1", "#2 补/v1 base（不拼接endpoint）");
        assert_eq!(final_urls[2], "http://api.test.com/chat/completions", "#3 原始base + endpoint");
        assert_eq!(final_urls[3], "http://api.test.com/v1/chat/completions", "#4 补/v1 base + endpoint");
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

    let error_msg = write_url_fallback_error_log(
        original_base,
        &base_candidates,
        &attempts,
        &final_error,
    );

    assert!(error_msg.contains("All 2 endpoint URL candidates failed"));
    assert!(error_msg.contains("http://wrong.endpoint.com"));
    assert!(error_msg.contains("~/.xiaoo/log/error.log"));
    assert!(error_msg.contains("http://wrong.endpoint.com/chat/completions"));
    println!("生成的错误消息:\n{}", error_msg);
}
}