use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;

pub use agent_types::LlmError;
use chrono::Local;
use regex::Regex;
use reqwest::StatusCode;
use serde_json::Value;

/// Credentials are scoped to one request, including response authentication headers.
#[derive(Default)]
pub(crate) struct ErrorRedactor {
    secrets: Vec<String>,
}

impl ErrorRedactor {
    pub(crate) fn new(url: &str, api_key: &str) -> Self {
        let mut redactor = Self::default();
        redactor.add(api_key);
        if let Ok(url) = reqwest::Url::parse(url) {
            redactor.add_userinfo(url.username());
            if let Some(password) = url.password() {
                redactor.add_userinfo(password);
            }
            for (_, value) in url.query_pairs() {
                redactor.add(&value);
            }
            // Preserve both wire-encoded and decoded credential spellings.
            if let Some(query) = url.query() {
                for pair in query.split('&') {
                    if let Some((_, value)) = pair.split_once('=') {
                        redactor.add(value);
                    }
                }
            }
        }
        redactor
    }

    fn add(&mut self, value: &str) {
        if value.is_empty() {
            return;
        }
        if !self.secrets.iter().any(|secret| secret == value) {
            self.secrets.push(value.to_string());
        }
        // SSE JSON error payloads can escape a known credential.
        if let Ok(encoded) = serde_json::to_string(value) {
            let escaped = &encoded[1..encoded.len() - 1];
            if !self.secrets.iter().any(|secret| secret == escaped) {
                self.secrets.push(escaped.to_string());
            }
        }
    }

    fn add_userinfo(&mut self, value: &str) {
        self.add(value);
        // Decode percent escapes without treating userinfo '+' as form whitespace.
        let escaped = value
            .replace('&', "%26")
            .replace('#', "%23")
            .replace('+', "%2B");
        if let Ok(url) = reqwest::Url::parse(&format!("http://localhost/?v={escaped}")) {
            if let Some((_, decoded)) = url.query_pairs().next() {
                self.add(&decoded);
            }
        }
    }

    pub(crate) fn headers(&mut self, headers: &reqwest::header::HeaderMap) {
        for (name, value) in headers {
            if Self::is_auth_header(name.as_str()) {
                if let Ok(value) = value.to_str() {
                    self.add(value);
                    if let Some(token) = value.strip_prefix("Bearer ") {
                        self.add(token);
                    }
                }
            }
        }
    }

    pub(crate) fn is_auth_header(name: &str) -> bool {
        matches!(
            name.to_ascii_lowercase().as_str(),
            "x-api-key" | "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
        )
    }

    pub(crate) fn text(&self, text: &str) -> String {
        let mut output = String::new();
        let mut rest = text;
        while !rest.is_empty() {
            let next = self
                .secrets
                .iter()
                .filter_map(|secret| rest.find(secret).map(|pos| (pos, secret.len())))
                .min_by_key(|&(pos, len)| (pos, std::cmp::Reverse(len)));
            let Some((pos, len)) = next else {
                output.push_str(rest);
                break;
            };
            output.push_str(&rest[..pos]);
            output.push_str("[REDACTED]");
            rest = &rest[pos + len..];
        }
        output
    }

    pub(crate) fn error_result<T>(&self, result: Result<T, LlmError>) -> Result<T, LlmError> {
        result.map_err(|error| self.error(error))
    }

    pub(crate) fn error(&self, mut error: LlmError) -> LlmError {
        let message = match &mut error {
            LlmError::RequestFailed { message }
            | LlmError::HttpError(message)
            | LlmError::ApiError(message)
            | LlmError::ParseError(message)
            | LlmError::RateLimited { message, .. }
            | LlmError::AuthError { message }
            | LlmError::ModelNotFound { model: message }
            | LlmError::ProviderNotFound(message)
            | LlmError::ConfigError(message)
            | LlmError::ContextLengthExceeded { message }
            | LlmError::StreamError { message }
            | LlmError::IoError(message) => message,
            LlmError::Timeout | LlmError::Cancelled => return error,
        };
        *message = self.text(message);
        error
    }
}

pub(crate) fn map_reqwest_error(err: reqwest::Error) -> LlmError {
    let redactor = ErrorRedactor::new(err.url().map(|url| url.as_str()).unwrap_or(""), "");
    let cause_chain = render_error_chain(&err);
    let error = if err.is_timeout() {
        LlmError::HttpError(format!(
            "Request timeout: {} | cause chain: {}",
            err, cause_chain
        ))
    } else if err.is_connect() {
        LlmError::HttpError(format!(
            "Connection failed: {} | cause chain: {}",
            err, cause_chain
        ))
    } else if err.is_decode() {
        LlmError::StreamError {
            message: format!(
                "error decoding response body: {} | cause chain: {}",
                err, cause_chain
            ),
        }
    } else {
        LlmError::HttpError(format!("{} | cause chain: {}", err, cause_chain))
    };
    redactor.error(error)
}

/// Walk `std::error::Error::source()` chain and join each level's `Display`
/// with ` -> `. reqwest's top-level `Display` is usually a generic
/// "error sending request for url (...)", while the actual root cause
/// (DNS, TCP, TLS handshake, proxy handshake) lives in the source chain.
/// Without this, callers only see the generic top-level message and cannot
/// tell TLS cert rejection from connection refused from DNS failure.
fn render_error_chain(err: &dyn std::error::Error) -> String {
    let mut chain = Vec::new();
    let mut current: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = current {
        let s = e.to_string();
        // Skip duplicates: many error wrappers just re-display their source.
        if chain
            .last()
            .is_none_or(|last: &String| last.as_str() != s.as_str())
        {
            chain.push(s);
        }
        current = e.source();
    }
    if chain.len() <= 1 {
        "(no underlying cause)".to_string()
    } else {
        chain.join(" -> ")
    }
}

pub(crate) fn map_serde_error(err: serde_json::Error) -> LlmError {
    LlmError::ParseError(err.to_string())
}

pub(crate) fn map_api_status_error(
    status: StatusCode,
    response_body: &str,
    _request_body: &str,
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

    let message = format_api_status_message(status, response_body);

    if is_context_overflow(status, response_body) {
        return LlmError::ContextLengthExceeded { message };
    }

    LlmError::ApiError(message)
}

fn format_api_status_message(status: StatusCode, response_body: &str) -> String {
    let body = if response_body.trim().is_empty() {
        "no response body"
    } else {
        response_body
    };
    format!("HTTP {status}: {body}")
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

/// 记录 LLM 流式请求失败的详细信息到 error.log（完整记录，不截断）
pub(crate) fn write_stream_error_log(
    url: &str,
    response_headers: Option<&reqwest::header::HeaderMap>,
    partial_response_buffer: &str,
    error_message: &str,
    http_status: Option<u16>,
) {
    let mut redactor = ErrorRedactor::new(url, "");
    if let Some(headers) = response_headers {
        redactor.headers(headers);
    }
    write_redacted_stream_error_log(
        url,
        response_headers,
        partial_response_buffer,
        error_message,
        http_status,
        &redactor,
    );
}

pub(crate) fn write_redacted_stream_error_log(
    url: &str,
    response_headers: Option<&reqwest::header::HeaderMap>,
    partial_response_buffer: &str,
    error_message: &str,
    http_status: Option<u16>,
    redactor: &ErrorRedactor,
) {
    let url = redactor.text(url);
    let partial_response_buffer = redactor.text(partial_response_buffer);
    let error_message = redactor.text(error_message);
    let log_path = get_error_log_path();

    if let Some(parent) = log_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!("Failed to create error log directory: {}", e);
            return;
        }
    }

    match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(mut file) => {
            let timestamp = Local::now().to_rfc3339();

            writeln!(file).ok();
            writeln!(file, "{}", "=".repeat(60)).ok();
            writeln!(file, "===== {} source=llm_stream_error =====", timestamp).ok();
            writeln!(file, "{}", "=".repeat(60)).ok();

            writeln!(file).ok();
            writeln!(file, "[Basic Info]").ok();
            writeln!(file, "URL: {}", url).ok();
            if let Some(status) = http_status {
                writeln!(file, "HTTP Status: {}", status).ok();
            }
            writeln!(file, "Error: {}", error_message).ok();

            if let Some(headers) = response_headers {
                writeln!(file).ok();
                writeln!(file, "[Key Headers]").ok();

                let essential_keys = ["content-type", "transfer-encoding"];
                let debug_keys = [
                    "x-request-id",
                    "retry-after",
                    "x-ratelimit-limit",
                    "x-ratelimit-remaining",
                    "x-ratelimit-reset",
                    "openai-model",
                ];

                for (key, value) in headers.iter() {
                    let key_str = key.as_str();
                    if essential_keys.contains(&key_str) || debug_keys.contains(&key_str) {
                        let value_str = value.to_str().unwrap_or("<binary>");
                        writeln!(file, "  {}: {}", key, redactor.text(value_str)).ok();
                    }
                }
            }

            writeln!(file).ok();
            writeln!(file, "[Response Body]").ok();
            writeln!(file, "Length: {} bytes", partial_response_buffer.len()).ok();
            if !partial_response_buffer.is_empty() {
                writeln!(file, "{}", partial_response_buffer).ok();
            }

            writeln!(file).ok();
            writeln!(file, "{}", "=".repeat(60)).ok();
            writeln!(file).ok();

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600))
                {
                    tracing::warn!("Failed to set error.log permissions: {}", e);
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to write stream error log: {}", e);
        }
    }
}

fn get_error_log_path() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".xiaoo").join("log").join("error.log"))
        .unwrap_or_else(|| PathBuf::from(".xiaoo_error.log"))
}

/// Structured details of a failed streaming LLM request, written to
/// error.log by [`write_stream_error_log`] as one complete block.
///
/// reqwest's top-level `Display` for body-read failures is the generic
/// "error decoding response body"; the actual root cause (read timeout,
/// chunked stream cut mid-transfer, ...) only exists in the error source
/// chain. Carrying the rendered chain plus the wire-level counters lets a
/// single log block answer "which request, how long did it stall, how many
/// bytes arrived, and why did it die" without re-reading provider source
/// code after the fact.
///
/// Sanitization: owned free-text fields are masked by
/// [`redact_credentials`] at construction (in [`map_stream_read_error`]
/// and [`map_protocol_mismatch`]). The borrowed SSE buffer cannot be
/// masked there — masking returns an owned String — so
/// [`write_stream_error_log`] masks it at the persistence boundary, where
/// an error page or SSE error event echoing the credential-bearing
/// request URL gets caught. With both boundaries covered, neither the log
/// nor the caller-facing `LlmError` built from the report leaks
/// query-string credentials (Gemini embeds its API key there).
struct StreamErrorReport<'a> {
    url: String,
    http_status: u16,
    response_headers: &'a reqwest::header::HeaderMap,
    /// Generic reqwest display message, e.g. "error decoding response body".
    error_message: String,
    /// Full `source()` chain rendered by [`render_error_chain`]; empty for
    /// protocol-mismatch failures, which have no underlying error object.
    cause_chain: String,
    /// `reqwest::Error::is_timeout()` captured at the failure site:
    /// distinguishes "endpoint stalled until the client timeout" from
    /// "stream cut mid-flight".
    timed_out: bool,
    /// Wall-clock ms from the start of this endpoint attempt to the
    /// failure; without it a 300s stall can only be reconstructed by
    /// diffing log timestamps after the fact.
    elapsed_ms: u64,
    /// Body bytes pulled off the wire before the failure; 0 means the
    /// endpoint never delivered a single byte (header-only stall).
    bytes_received: usize,
    /// Leftover SSE line buffer not yet consumed when the stream died;
    /// logged as "Unconsumed bytes" so it cannot be misread as the total
    /// received count ([`Self::bytes_received`] is that number).
    /// Server-controlled, hence masked at the persistence boundary (see
    /// the struct-level sanitization note).
    unconsumed_line_buffer: &'a str,
}

/// Single entry for a `bytes_stream()` read failure: writes the structured
/// report to error.log and returns the caller-facing `StreamError` with
/// the root-cause chain inline. reqwest's top-level `Display` for these
/// failures is the generic "error decoding response body"; without the
/// chain callers cannot tell a read timeout from a mid-flight stream cut.
/// `bytes_received` counts body bytes read before the failing chunk, so 0
/// means a header-only stall. Consolidated here so every provider reports
/// identically instead of hand-rolling the report/log/message trio.
pub(crate) fn map_stream_read_error(
    url: &str,
    redactor: &ErrorRedactor,
    http_status: u16,
    response_headers: &reqwest::header::HeaderMap,
    error: &reqwest::Error,
    started: std::time::Instant,
    bytes_received: usize,
    unconsumed_line_buffer: &str,
) -> LlmError {
    let report = StreamErrorReport {
        url: redactor.text(&redact_credentials(url)),
        http_status,
        response_headers,
        error_message: redactor.text(&redact_credentials(&error.to_string())),
        cause_chain: redactor.text(&redact_credentials(&render_error_chain(error))),
        timed_out: error.is_timeout(),
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_received,
        unconsumed_line_buffer,
    };
    let message = match write_structured_stream_error_log(&report, redactor) {
        Some(log_path) => format!(
            "{} | cause chain: {} (详见 {})",
            report.error_message,
            report.cause_chain,
            log_path.display()
        ),
        // The log block could not be persisted, so pointing at the file
        // would mislead the reader; the chain itself still carries the
        // root cause.
        None => format!(
            "{} | cause chain: {}",
            report.error_message, report.cause_chain
        ),
    };
    LlmError::StreamError {
        // The chain rides along so trace spans and the caller's error
        // message carry the root cause, not just reqwest's generic
        // "error decoding response body" wrapper.
        message,
    }
}

/// Single entry for a response that completed at the HTTP layer but is not
/// the expected streaming protocol (e.g. a non-SSE content type): writes
/// the report to error.log and returns the caller-facing `ApiError`. No
/// reqwest error object exists here, so the cause chain is empty and the
/// timeout flag is meaningless.
pub(crate) fn map_protocol_mismatch(
    url: &str,
    redactor: &ErrorRedactor,
    http_status: u16,
    response_headers: &reqwest::header::HeaderMap,
    error_message: String,
    started: std::time::Instant,
    body: &str,
) -> LlmError {
    let report = StreamErrorReport {
        url: redactor.text(&redact_credentials(url)),
        http_status,
        response_headers,
        error_message: redactor.text(&redact_credentials(&error_message)),
        cause_chain: String::new(),
        timed_out: false,
        elapsed_ms: started.elapsed().as_millis() as u64,
        bytes_received: body.len(),
        unconsumed_line_buffer: body,
    };
    let message = match write_structured_stream_error_log(&report, redactor) {
        Some(log_path) => format!("{} (详见 {})", report.error_message, log_path.display()),
        None => report.error_message,
    };
    LlmError::ApiError(message)
}

/// Log the full details of a failed streaming LLM request to error.log
/// (complete, never truncated). Returns `Some(path)` only when the block
/// was actually persisted, so caller-facing messages never point at a file
/// that is missing the report — XIAOO_ERROR_LOG may also have redirected
/// the file away from the default per-user location, so the returned path
/// is the real one, not a hardcoded default.
fn write_structured_stream_error_log(
    report: &StreamErrorReport<'_>,
    redactor: &ErrorRedactor,
) -> Option<PathBuf> {
    let log_path = error_log_path();

    if let Some(parent) = log_path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            tracing::error!("Failed to create error log directory: {}", e);
            return None;
        }
    }

    let mut file = match OpenOptions::new().create(true).append(true).open(&log_path) {
        Ok(file) => file,
        Err(e) => {
            tracing::error!("Failed to write stream error log: {}", e);
            return None;
        }
    };

    // One writeln failure is reported once at the end; per-line `.ok()`
    // would let a broken pipe fail silently line after line.
    let mut first_write_error: Option<std::io::Error> = None;
    macro_rules! line {
        ($($arg:tt)*) => {
            if let Err(e) = writeln!(file, $($arg)*) {
                first_write_error.get_or_insert(e);
            }
        };
    }

    let timestamp = Local::now().to_rfc3339();
    line!();
    line!("{}", "=".repeat(60));
    line!("===== {} source=llm_stream_error =====", timestamp);
    line!("{}", "=".repeat(60));

    line!();
    line!("[Basic Info]");
    line!("URL: {}", report.url);
    line!("HTTP Status: {}", report.http_status);
    line!("Timed Out: {}", report.timed_out);
    line!("Elapsed: {} ms", report.elapsed_ms);
    line!("Bytes Received: {}", report.bytes_received);
    line!("Error: {}", report.error_message);
    if !report.cause_chain.is_empty() {
        line!("Cause Chain: {}", report.cause_chain);
    }

    line!();
    line!("[Key Headers]");

    let essential_keys = ["content-type", "transfer-encoding"];
    // Response headers only: x-api-key is a request header, and a
    // server that echoes it back would turn this log into a leak.
    let debug_keys = [
        "x-request-id",
        "retry-after",
        "x-ratelimit-limit",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
        "openai-model",
    ];

    for (key, value) in report.response_headers.iter() {
        let key_str = key.as_str();
        if essential_keys.contains(&key_str) || debug_keys.contains(&key_str) {
            let value_str = value.to_str().unwrap_or("<binary>");
            line!("  {}: {}", key, redactor.text(value_str));
        }
    }

    line!();
    line!("[Stream Buffer]");
    // The count is a wire stat on the raw buffer and stays unmasked; only
    // the rendered text is masked (see StreamErrorReport's sanitization
    // note).
    line!("Unconsumed bytes: {}", report.unconsumed_line_buffer.len());
    if !report.unconsumed_line_buffer.is_empty() {
        line!(
            "{}",
            redactor.text(&redact_credentials(report.unconsumed_line_buffer))
        );
    }

    line!();
    line!("{}", "=".repeat(60));
    line!();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600))
        {
            tracing::warn!("Failed to set error.log permissions: {}", e);
        }
    }

    if let Some(e) = first_write_error {
        tracing::error!("Failed to write stream error log: {}", e);
        return None;
    }
    Some(log_path)
}

#[cfg(test)]
#[path = "../../../tests/unit/llm-client/error_test.rs"]
mod tests;
