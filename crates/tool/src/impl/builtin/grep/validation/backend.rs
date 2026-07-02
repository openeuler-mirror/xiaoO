use super::constants::max_timeout_ms;
use super::input::GrepInput;

pub mod error_code {
    pub const PATTERN_EMPTY: u32 = 1;
    #[allow(dead_code)]
    pub const PATH_NOT_FOUND: u32 = 2;
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
mod tests {
    use super::*;

    fn input_with_timeout(timeout: Option<u64>) -> GrepInput {
        GrepInput {
            pattern: "x".to_string(),
            timeout,
            ..Default::default()
        }
    }

    #[test]
    fn timeout_none_is_ok() {
        assert!(validate_timeout(&input_with_timeout(None)).result);
    }

    #[test]
    fn timeout_zero_is_invalid() {
        let result = validate_timeout(&input_with_timeout(Some(0)));
        assert!(!result.result);
        assert_eq!(result.error_code, Some(error_code::TIMEOUT_INVALID));
    }

    #[test]
    fn timeout_above_max_is_rejected() {
        let result = validate_timeout(&input_with_timeout(Some(max_timeout_ms() + 1)));
        assert!(!result.result);
        assert_eq!(result.error_code, Some(error_code::TIMEOUT_EXCEEDS_MAX));
        assert!(result
            .message
            .as_ref()
            .is_some_and(|m| m.contains("exceeds maximum allowed")));
    }

    #[test]
    fn timeout_at_max_is_ok() {
        assert!(validate_timeout(&input_with_timeout(Some(max_timeout_ms()))).result);
    }

    #[test]
    fn validate_input_reaches_timeout_check_with_valid_pattern() {
        // Pattern/path are valid here, so the timeout branch must be reached.
        let result = validate_input(&input_with_timeout(Some(0)));
        assert!(!result.result);
        assert_eq!(result.error_code, Some(error_code::TIMEOUT_INVALID));
    }

    #[test]
    fn validate_input_pattern_precedence_over_timeout() {
        // Empty pattern must win over a bad timeout so callers get the most
        // actionable error first.
        let result = validate_input(&GrepInput {
            pattern: "   ".to_string(),
            timeout: Some(0),
            ..Default::default()
        });
        assert!(!result.result);
        assert_eq!(result.error_code, Some(error_code::PATTERN_EMPTY));
    }
}
