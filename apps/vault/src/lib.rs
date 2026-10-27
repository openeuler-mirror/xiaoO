//! Vault Plugin
//!
//! Key and encryption management:
//! - KeyProvider: key provider abstraction
//! - InstanceKey: per-instance random master key (software default)
//! - WhiteBox: legacy v1 derivation, decrypt-only for migration
//! - SDF: national crypto interface (incl. TEE key providers)

pub mod instance_key;
pub mod key_provider;
pub mod key_provider_error;
pub mod sdf;
pub mod types;
pub mod whitebox;

// Re-export key provider types
pub use key_provider::{KeyMaterial, KeyProvider};
pub use key_provider_error::KeyProviderError;

// Re-export providers
pub use instance_key::{store_key_tag, InstanceKeyProvider};
pub use sdf::{
    decrypt_secret, encrypt_secret, init_sdf_provider, SdfKeyProvider, TeeKeyProvider, TeeType,
    DEFAULT_SDF_LIBRARY_PATH,
};
pub use whitebox::legacy_v1_master_key;
