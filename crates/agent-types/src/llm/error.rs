#[derive(Clone, Debug, thiserror::Error)]
pub enum LlmError {
    #[error("request failed: {message}")]
    RequestFailed { message: String },

    #[error("HTTP error: {0}")]
    HttpError(String),

    #[error("API error: {0}")]
    ApiError(String),

    #[error("parse error: {0}")]
    ParseError(String),

    #[error("rate limited: retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },

    #[error("authentication error: {message}")]
    AuthError { message: String },

    #[error("model not found: {model}")]
    ModelNotFound { model: String },

    #[error("provider not found: {0}")]
    ProviderNotFound(String),

    #[error("configuration error: {0}")]
    ConfigError(String),

    #[error("context length exceeded: {message}")]
    ContextLengthExceeded { message: String },

    #[error("stream error: {message}")]
    StreamError { message: String },

    #[error("IO error: {0}")]
    IoError(String),

    #[error("timeout")]
    Timeout,

    #[error("cancelled")]
    Cancelled,
}

impl From<std::io::Error> for LlmError {
    fn from(e: std::io::Error) -> Self {
        LlmError::IoError(e.to_string())
    }
}
