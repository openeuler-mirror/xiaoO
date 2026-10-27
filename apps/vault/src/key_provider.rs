//! Key Provider - key derivation abstraction
//!
//! - InstanceKey: per-instance random master key for software environments
//! - Tee: TEE (Trusted Execution Environment)
//! - Hsm: HSM (Hardware Security Module)

pub use crate::key_provider_error::KeyProviderError;
pub use crate::types::KeyMaterial;

use async_trait::async_trait;

/// Key provider interface.
#[async_trait]
pub trait KeyProvider: Send + Sync {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError>;

    /// Provider type identifier.
    fn provider_type(&self) -> &'static str;

    /// Provider name (for logs and configuration).
    fn name(&self) -> &str;
}
