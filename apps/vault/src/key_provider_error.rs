use thiserror::Error;

#[derive(Error, Debug)]
pub enum KeyProviderError {
    #[error("tee: {0}")]
    Tee(String),

    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("provider not available: {0}")]
    ProviderNotAvailable(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}
