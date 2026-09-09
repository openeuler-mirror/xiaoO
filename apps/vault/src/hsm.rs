//! HSM Key Provider
//!
//! 通过 PKCS#11 接口从 HSM 获取密钥

use crate::key_provider::{KeyMaterial, KeyProvider, KeyProviderError};
use async_trait::async_trait;

/// HSM 密钥提供者
///
/// 通过 PKCS#11 接口访问 HSM 中的密钥
pub struct HsmKeyProvider {
    name: String,
    _library_path: String,
    _slot_id: String,
    _key_label: String,
}

impl HsmKeyProvider {
    pub fn new(
        name: impl Into<String>,
        library_path: impl Into<String>,
        slot_id: impl Into<String>,
        key_label: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            _library_path: library_path.into(),
            _slot_id: slot_id.into(),
            _key_label: key_label.into(),
        }
    }

    pub fn with_library(name: impl Into<String>, library_path: impl Into<String>) -> Self {
        Self::new(
            name,
            library_path,
            "0".to_string(),
            "xiaoo_master_key".to_string(),
        )
    }
}

#[async_trait]
impl KeyProvider for HsmKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "HSM/PKCS#11 provider is not linked in this build".into(),
        ))
    }

    fn provider_type(&self) -> &'static str {
        "hsm"
    }

    fn name(&self) -> &str {
        &self.name
    }
}
