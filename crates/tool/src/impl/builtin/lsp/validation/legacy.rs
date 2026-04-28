#![allow(unused)]
use super::input::LspInput;

pub mod error_code {
    pub const FILE_PATH_EMPTY: u32 = 1;
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

pub fn validate_input(input: &LspInput) -> ValidationResult {
    if input.file_path.trim().is_empty() {
        return ValidationResult::error(
            "file_path cannot be empty",
            error_code::FILE_PATH_EMPTY,
        );
    }
    ValidationResult::ok()
}
