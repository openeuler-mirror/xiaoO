use thiserror::Error;

#[derive(Error, Debug)]
pub enum KeyProviderError {
    #[error("whitebox: {0}")]
    WhiteBox(String),

    #[error("tee: {0}")]
    Tee(String),

    #[error("hsm: {0}")]
    Hsm(String),

    #[error("key not found: {0}")]
    KeyNotFound(String),

    #[error("invalid key length: expected {expected}, got {got}")]
    InvalidKeyLength { expected: usize, got: usize },

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("provider not available: {0}")]
    ProviderNotAvailable(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("hex decode error: {0}")]
    HexDecode(#[from] hex::FromHexError),
}