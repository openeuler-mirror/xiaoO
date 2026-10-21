use super::constants::MAX_NUM_RESULTS;
use super::input::WebSearchInput;

pub mod error_code {
    pub const QUERY_EMPTY: u32 = 1001;
    pub const NUM_RESULTS_OUT_OF_RANGE: u32 = 1002;
    pub const CONTEXT_MAX_CHARACTERS_EMPTY: u32 = 1003;
}

pub struct ValidationResult {
    pub result: bool,
    pub message: Option<String>,
    pub error_code: Option<u32>,
}

pub fn validate_input(input: &WebSearchInput) -> ValidationResult {
    if input.query.trim().is_empty() {
        return ValidationResult {
            result: false,
            message: Some("query must not be empty".to_string()),
            error_code: Some(error_code::QUERY_EMPTY),
        };
    }

    if let Some(num_results) = input.num_results {
        if num_results == 0 || num_results > MAX_NUM_RESULTS {
            return ValidationResult {
                result: false,
                message: Some(format!(
                    "num_results must be between 1 and {} (got {})",
                    MAX_NUM_RESULTS, num_results
                )),
                error_code: Some(error_code::NUM_RESULTS_OUT_OF_RANGE),
            };
        }
    }

    if let Some(context_max_characters) = input.context_max_characters {
        if context_max_characters == 0 {
            return ValidationResult {
                result: false,
                message: Some("context_max_characters must be greater than 0".to_string()),
                error_code: Some(error_code::CONTEXT_MAX_CHARACTERS_EMPTY),
            };
        }
    }

    ValidationResult {
        result: true,
        message: None,
        error_code: None,
    }
}
