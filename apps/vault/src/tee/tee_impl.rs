//! TEE Key Provider 实现
//!
//! 从 TEE 安全内存中获取密钥
//! 支持: Intel SGX, ARM TrustZone, AWS Nitro Enclave, Intel TDX, SDF 国密

use crate::key_provider::{KeyMaterial, KeyProvider, KeyProviderError};
use async_trait::async_trait;

#[derive(Clone, Debug)]
pub enum TeeType {
    IntelSgx,
    ArmTrustZone,
    Hypervisor,
    AwsNitro,
    IntelTdx,
    Sdf,
    Custom(String),
}

impl TeeType {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "intel_sgx" | "sgx" => Self::IntelSgx,
            "arm_trustzone" | "trustzone" | "tz" => Self::ArmTrustZone,
            "hypervisor" | "hvm" => Self::Hypervisor,
            "aws_nitro" | "nitro" => Self::AwsNitro,
            "intel_tdx" | "tdx" => Self::IntelTdx,
            "sdf" | "national_security_module" => Self::Sdf,
            other => Self::Custom(other.to_string()),
        }
    }
}

/// TEE 密钥提供者
pub struct TeeKeyProvider {
    name: String,
    tee_type: TeeType,
    slot: Option<String>,
}

impl TeeKeyProvider {
    pub fn new(name: impl Into<String>, tee_type: TeeType) -> Self {
        Self {
            name: name.into(),
            tee_type,
            slot: None,
        }
    }

    pub fn with_slot(mut self, slot: impl Into<String>) -> Self {
        self.slot = Some(slot.into());
        self
    }

    pub fn intel_sgx(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::IntelSgx)
    }

    pub fn arm_trustzone(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::ArmTrustZone)
    }

    pub fn aws_nitro(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::AwsNitro)
    }

    pub fn sdf(name: impl Into<String>) -> Self {
        Self::new(name, TeeType::Sdf)
    }
}

#[async_trait]
impl KeyProvider for TeeKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        match self.tee_type {
            TeeType::IntelSgx => self.get_from_sgx().await,
            TeeType::ArmTrustZone => self.get_from_trustzone().await,
            TeeType::AwsNitro => self.get_from_nitro().await,
            TeeType::Hypervisor => self.get_from_hypervisor().await,
            TeeType::IntelTdx => self.get_from_tdx().await,
            TeeType::Sdf => self.get_from_sdf().await,
            TeeType::Custom(ref t) => Err(KeyProviderError::Tee(
                format!("unsupported TEE type: {}", t)
            )),
        }
    }

    fn provider_type(&self) -> &'static str {
        "tee"
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl TeeKeyProvider {
    async fn get_from_sgx(&self) -> Result<KeyMaterial, KeyProviderError> {
        #[cfg(feature = "tee_sgx")]
        {
            return self.sgx_provision().await;
        }

        Err(KeyProviderError::ProviderNotAvailable(
            "Intel SGX not available. Compile with feature 'tee_sgx' to enable.".into()
        ))
    }

    async fn get_from_trustzone(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "ARM TrustZone not yet implemented".into()
        ))
    }

    async fn get_from_nitro(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "AWS Nitro Enclave not yet implemented".into()
        ))
    }

    async fn get_from_hypervisor(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "Hypervisor secure world not yet implemented".into()
        ))
    }

    async fn get_from_tdx(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "Intel TDX not yet implemented".into()
        ))
    }

    #[cfg(feature = "tee_sdf")]
    async fn get_from_sdf(&self) -> Result<KeyMaterial, KeyProviderError> {
        use super::sdf::SdfKeyProvider;

        let provider = SdfKeyProvider::new(self.name(), "/usr/local/sdf/lib/libsdf.so");
        provider.provide().await
    }

    #[cfg(not(feature = "tee_sdf"))]
    async fn get_from_sdf(&self) -> Result<KeyMaterial, KeyProviderError> {
        Err(KeyProviderError::ProviderNotAvailable(
            "SDF (National Security Module) not available. Compile with feature 'tee_sdf' to enable.".into()
        ))
    }

    #[cfg(feature = "tee_sgx")]
    async fn sgx_provision(&self) -> Result<KeyMaterial, KeyProviderError> {
        use std::sync::OnceLock;

        static ENCLAVE_KEY: OnceLock<[u8; 32]> = OnceLock::new();

        let key = ENCLAVE_KEY.get_or_try_init(|| {
            Err(KeyProviderError::Tee("SGX key not yet initialized".into()))
        })?;

        Ok(KeyMaterial::new(*key, format!("tee_sgx_{}", self.name())))
    }
}