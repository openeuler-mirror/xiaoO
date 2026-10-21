use agent_contracts::backend::{PathKind, PathStat};

#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub result: bool,
    pub message: Option<String>,
}

impl ValidationResult {
    pub fn ok() -> Self {
        Self {
            result: true,
            message: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            result: false,
            message: Some(message.into()),
        }
    }
}

pub fn validate_path_shape(path: &str) -> ValidationResult {
    if path.starts_with("\\\\") || path.starts_with("//") {
        return ValidationResult::error("UNC paths are not allowed for backend glob execution");
    }

    ValidationResult::ok()
}

pub fn validate_base_dir(path: &str, stat: &PathStat) -> ValidationResult {
    if !stat.exists {
        return ValidationResult::error(format!("Directory does not exist: {}", path));
    }

    if stat.kind != Some(PathKind::Directory) {
        return ValidationResult::error(format!("Path is not a directory: {}", path));
    }

    ValidationResult::ok()
}
