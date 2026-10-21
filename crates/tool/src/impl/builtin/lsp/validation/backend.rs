use super::input::LspInput;

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

pub fn validate_input(input: &LspInput) -> ValidationResult {
    if input.file_path.trim().is_empty() {
        return ValidationResult::error("file_path cannot be empty");
    }
    ValidationResult::ok()
}
