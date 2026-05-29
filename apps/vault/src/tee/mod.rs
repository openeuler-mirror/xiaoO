//! TEE 模块
//!
//! 包含多种 TEE 密钥提供者实现

pub mod tee_impl;
pub mod sdf;

pub use tee_impl::{TeeKeyProvider, TeeType};

#[cfg(feature = "tee_sdf")]
pub use sdf::{SdfKeyProvider, init_sdf_provider, encrypt_secret, decrypt_secret};

#[cfg(not(feature = "tee_sdf"))]
pub use sdf::{SdfKeyProvider};