use std::path::Path;

use crate::r#impl::path_resolver::expand_path_from_base;

use super::constants::max_timeout_ms;
use super::input::BashInput;

pub mod error_code {
    pub const COMMAND_EMPTY: u32 = 1;
    pub const CWD_NOT_FOUND: u32 = 2;
    pub const CWD_NOT_DIRECTORY: u32 = 3;
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

fn validate_command(input: &BashInput) -> ValidationResult {
    if input.command.trim().is_empty() {
        return ValidationResult::error("Command cannot be empty", error_code::COMMAND_EMPTY);
    }

    ValidationResult::ok()
}

fn validate_cwd(input: &BashInput, base_dir: &Path) -> ValidationResult {
    let Some(cwd) = input.cwd.as_deref() else {
        return ValidationResult::ok();
    };

    let resolved_cwd = expand_path_from_base(cwd, base_dir);
    let path = Path::new(&resolved_cwd);
    if !path.exists() {
        return ValidationResult::error(
            format!("Working directory does not exist: {}", cwd),
            error_code::CWD_NOT_FOUND,
        );
    }

    if !path.is_dir() {
        return ValidationResult::error(
            format!("Working directory is not a directory: {}", cwd),
            error_code::CWD_NOT_DIRECTORY,
        );
    }

    ValidationResult::ok()
}

fn validate_timeout(input: &BashInput) -> ValidationResult {
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

pub fn validate_input_with_base(input: &BashInput, base_dir: &Path) -> ValidationResult {
    let command_result = validate_command(input);
    if !command_result.result {
        return command_result;
    }

    let cwd_result = validate_cwd(input, base_dir);
    if !cwd_result.result {
        return cwd_result;
    }

    let timeout_result = validate_timeout(input);
    if !timeout_result.result {
        return timeout_result;
    }

    ValidationResult::ok()
}
