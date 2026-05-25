pub mod error_code {
    pub const DIRECTORY_NOT_FOUND: u32 = 1;
    pub const NOT_A_DIRECTORY: u32 = 2;
    pub const UNC_PATH_BLOCKED: u32 = 3;
}

use agent_contracts::backend::{PathKind, PathStat};

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

pub fn validate_path_shape(path: &str) -> ValidationResult {
    if path.starts_with("\\\\") || path.starts_with("//") {
        return ValidationResult::error(
            "UNC paths are not allowed for backend glob execution",
            error_code::UNC_PATH_BLOCKED,
        );
    }

    ValidationResult::ok()
}

pub fn validate_base_dir(path: &str, stat: &PathStat) -> ValidationResult {
    if !stat.exists {
        return ValidationResult::error(
            format!("Directory does not exist: {}", path),
            error_code::DIRECTORY_NOT_FOUND,
        );
    }

    if stat.kind != Some(PathKind::Directory) {
        return ValidationResult::error(
            format!("Path is not a directory: {}", path),
            error_code::NOT_A_DIRECTORY,
        );
    }

    ValidationResult::ok()
}
