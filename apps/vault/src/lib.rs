//! Vault Plugin
//!
//! 统一的密钥和加密管理插件，包含:
//! - KeyProvider: 密钥提供者抽象接口
//! - WhiteBox: 白盒密钥实现
//! - SDF: 国密接口 (包含 TEE 密钥提供者)
//! - HSM: 硬件安全模块密钥接口

pub mod key_provider;
pub mod types;
pub mod key_provider_error;
pub mod whitebox;
pub mod sdf;
pub mod hsm;

// Re-export key provider types
pub use key_provider::{KeyProvider, KeyMaterial, KeyProviderConfig};
pub use key_provider_error::KeyProviderError;

// Re-export providers
pub use whitebox::WhiteBoxKeyProvider;
pub use sdf::{TeeKeyProvider, TeeType, SdfKeyProvider, init_sdf_provider, encrypt_secret, decrypt_secret};
pub use hsm::HsmKeyProvider;
