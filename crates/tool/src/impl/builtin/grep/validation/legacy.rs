use std::path::Path;

use crate::r#impl::path_resolver::expand_path_from_base;

use super::input::GrepInput;

pub mod error_code {
    pub const PATTERN_EMPTY: u32 = 1;
    pub const PATH_NOT_FOUND: u32 = 2;
    pub const UNC_PATH_BLOCKED: u32 = 3;
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

pub fn expand_path(path: &str, base_dir: &Path) -> String {
    let path = path.trim();

    if is_unc_path(path) {
        return path.to_string();
    }

    expand_path_from_base(path, base_dir)
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

pub fn validate_path(input: &GrepInput, base_dir: &Path) -> ValidationResult {
    if let Some(ref path) = input.path {
        let absolute_path = expand_path(path, base_dir);

        if is_unc_path(&absolute_path) {
            return ValidationResult::error(
                "UNC paths are not allowed for security reasons (NTLM credential leak prevention)",
                error_code::UNC_PATH_BLOCKED,
            );
        }

        if Path::new(&absolute_path).exists() {
            ValidationResult::ok()
        } else {
            ValidationResult::error(
                format!("Path does not exist: {}", path),
                error_code::PATH_NOT_FOUND,
            )
        }
    } else {
        ValidationResult::ok()
    }
}

pub fn validate_input_with_base(input: &GrepInput, base_dir: &Path) -> ValidationResult {
    let pattern_result = validate_pattern(input);
    if !pattern_result.result {
        return pattern_result;
    }

    let path_result = validate_path(input, base_dir);
    if !path_result.result {
        return path_result;
    }

    ValidationResult::ok()
}
