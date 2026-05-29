//! Key Provider - 密钥派生抽象层
//! 
//! 提供三种密钥派生方式:
//! - WhiteBox: 适合无硬件安全环境
//! - Tee: 适合 TEE (Trust Execution Environment) 环境
//! - Hsm: 适合 HSM (Hardware Security Module) 环境

pub use crate::key_provider_error::KeyProviderError;
pub use crate::types::{KeyMaterial, KeyProviderConfig};

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