use serde_json::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct OperationBackendConfig {
    /// Backend family identifier, for example local / docker / remote.
    pub kind: String,
    /// Implementation-specific structured configuration payload.
    pub options: Value,
}

impl OperationBackendConfig {
    pub fn new(kind: impl Into<String>, options: Value) -> Self {
        Self {
            kind: kind.into(),
            options,
        }
    }
}
