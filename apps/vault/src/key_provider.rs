//! Key Provider - key derivation abstraction
//!
//! - InstanceKey: per-instance random master key for software environments
//! - Tee: TEE (Trusted Execution Environment)
//! - Hsm: HSM (Hardware Security Module)

pub use crate::key_provider_error::KeyProviderError;
pub use crate::types::KeyMaterial;

use async_trait::async_trait;

/// 密钥提供者接口
///
/// 实现此 trait 来提供密钥材料
#[async_trait]
pub trait KeyProvider: Send + Sync {
    /// 获取密钥材料
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError>;

    /// 提供者类型标识
    fn provider_type(&self) -> &'static str;

    /// 提供者名称 (用于日志和配置)
    fn name(&self) -> &str;
}
