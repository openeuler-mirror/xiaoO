pub use agent_types::LlmError;

pub(crate) fn map_reqwest_error(err: reqwest::Error) -> LlmError {
    if err.is_timeout() {
        LlmError::HttpError(format!("Request timeout: {}", err))
    } else if err.is_connect() {
        LlmError::HttpError(format!("Connection failed: {}", err))
    } else {
        LlmError::HttpError(err.to_string())
    }
}

pub(crate) fn map_serde_error(err: serde_json::Error) -> LlmError {
    LlmError::ParseError(err.to_string())
}
