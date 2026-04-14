#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("missing required field: {field}")]
    MissingRequiredField { field: String },

    #[error("invalid config: {message}")]
    InvalidConfig { message: String },

    #[error("dependency error: {message}")]
    DependencyError { message: String },
}
