//! Vault Plugin
//!
//! 统一的密钥和加密管理插件，包含:
//! - KeyProvider: 密钥提供者抽象接口
//! - WhiteBox: 白盒密钥实现
//! - SDF: 国密接口 (包含 TEE 密钥提供者)
//! - HSM: 硬件安全模块密钥接口

pub mod hsm;
pub mod key_provider;
pub mod key_provider_error;
pub mod sdf;
pub mod types;
pub mod whitebox;

// Re-export key provider types
pub use key_provider::{KeyMaterial, KeyProvider, KeyProviderConfig};
pub use key_provider_error::KeyProviderError;

// Re-export providers
pub use hsm::HsmKeyProvider;
pub use sdf::{
    decrypt_secret, encrypt_secret, init_sdf_provider, SdfKeyProvider, TeeKeyProvider, TeeType,
};
pub use whitebox::WhiteBoxKeyProvider;
