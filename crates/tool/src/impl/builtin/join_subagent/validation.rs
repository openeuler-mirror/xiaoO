use super::input::JoinSubagentInput;

pub mod error_code {
    pub const TARGET_AGENT_ID_EMPTY: u32 = 1;
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

pub fn validate_input(input: &JoinSubagentInput) -> ValidationResult {
    if input.target_agent_id.0.trim().is_empty() {
        return ValidationResult::error(
            "target_agent_id must not be empty",
            error_code::TARGET_AGENT_ID_EMPTY,
        );
    }

    ValidationResult::ok()
}
