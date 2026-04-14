use super::constants::max_timeout_ms;
use super::input::WebFetchInput;

pub mod error_code {
    pub const URL_EMPTY: u32 = 1;
    pub const URL_INVALID_SCHEME: u32 = 2;
    pub const TIMEOUT_INVALID: u32 = 3;
    pub const TIMEOUT_EXCEEDS_MAX: u32 = 4;
}

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub result: bool,
    pub message: Option<String>,
    pub error_code: Option<u32>,
}

impl ValidationResult {
    pub fn ok() -> Self {
        Self {
            result: true,
            message: None,
            error_code: None,
        }
    }

    pub fn error(message: impl Into<String>, error_code: u32) -> Self {
        Self {
            result: false,
            message: Some(message.into()),
            error_code: Some(error_code),
        }
    }
}

fn validate_url(input: &WebFetchInput) -> ValidationResult {
    if input.url.trim().is_empty() {
        return ValidationResult::error("URL cannot be empty", error_code::URL_EMPTY);
    }

    if !input.url.starts_with("http://") && !input.url.starts_with("https://") {
        return ValidationResult::error(
            format!(
                "URL must start with http:// or https://, got: {}",
                input.url
            ),
            error_code::URL_INVALID_SCHEME,
        );
    }

    ValidationResult::ok()
}

fn validate_timeout(input: &WebFetchInput) -> ValidationResult {
    let Some(timeout) = input.timeout else {
        return ValidationResult::ok();
    };

    if timeout == 0 {
        return ValidationResult::error(
            "Timeout must be greater than 0 milliseconds",
            error_code::TIMEOUT_INVALID,
        );
    }

    let max_timeout = max_timeout_ms();
    if timeout > max_timeout {
        return ValidationResult::error(
            format!(
                "Timeout {}ms exceeds maximum allowed {}ms",
                timeout, max_timeout
            ),
            error_code::TIMEOUT_EXCEEDS_MAX,
        );
    }

    ValidationResult::ok()
}

pub fn validate_input(input: &WebFetchInput) -> ValidationResult {
    let url_result = validate_url(input);
    if !url_result.result {
        return url_result;
    }

    let timeout_result = validate_timeout(input);
    if !timeout_result.result {
        return timeout_result;
    }

    ValidationResult::ok()
}
