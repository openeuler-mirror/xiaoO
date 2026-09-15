use super::constants::max_timeout_ms;
use super::input::GrepInput;

pub mod error_code {
    pub const PATTERN_EMPTY: u32 = 1;
    pub const UNC_PATH_BLOCKED: u32 = 3;
    pub const TIMEOUT_INVALID: u32 = 4;
    pub const TIMEOUT_EXCEEDS_MAX: u32 = 5;
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

pub fn validate_pattern(input: &GrepInput) -> ValidationResult {
    if input.pattern.trim().is_empty() {
        return ValidationResult::error("Pattern cannot be empty", error_code::PATTERN_EMPTY);
    }
    ValidationResult::ok()
}

pub fn is_unc_path(path: &str) -> bool {
    path.starts_with("\\\\") || path.starts_with("//")
}

pub fn validate_path(input: &GrepInput) -> ValidationResult {
    if let Some(ref path) = input.path {
        if is_unc_path(path.trim()) {
            return ValidationResult::error(
                "UNC paths are not allowed for security reasons (NTLM credential leak prevention)",
                error_code::UNC_PATH_BLOCKED,
            );
        }
    }

    ValidationResult::ok()
}

pub fn validate_timeout(input: &GrepInput) -> ValidationResult {
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

pub fn validate_input(input: &GrepInput) -> ValidationResult {
    let pattern_result = validate_pattern(input);
    if !pattern_result.result {
        return pattern_result;
    }

    let path_result = validate_path(input);
    if !path_result.result {
        return path_result;
    }

    let timeout_result = validate_timeout(input);
    if !timeout_result.result {
        return timeout_result;
    }

    ValidationResult::ok()
}

#[cfg(test)]
#[path = "../../../../../../../tests/unit/tool/impl/builtin/grep/validation/backend_test.rs"]
mod tests;
