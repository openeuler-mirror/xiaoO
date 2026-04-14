use super::input::SpawnSubagentInput;

pub mod error_code {
    pub const DESCRIPTION_EMPTY: u32 = 1;
    pub const PROMPT_EMPTY: u32 = 2;
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

pub fn validate_input(input: &SpawnSubagentInput) -> ValidationResult {
    if input.description.trim().is_empty() {
        return ValidationResult::error(
            "description must not be empty",
            error_code::DESCRIPTION_EMPTY,
        );
    }

    if input.prompt.trim().is_empty() {
        return ValidationResult::error("prompt must not be empty", error_code::PROMPT_EMPTY);
    }

    ValidationResult::ok()
}
