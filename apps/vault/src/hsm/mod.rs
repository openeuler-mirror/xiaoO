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
    library_path: String,
    slot_id: String,
    key_label: String,
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
            library_path: library_path.into(),
            slot_id: slot_id.into(),
            key_label: key_label.into(),
        }
    }
    
    pub fn with_library(name: impl Into<String>, library_path: impl Into<String>) -> Self {
        Self::new(
            name, 
            library_path, 
            "0".to_string(), 
            "xiaoo_master_key".to_string()
        )
    }
}

#[async_trait]
impl KeyProvider for HsmKeyProvider {
    async fn provide(&self) -> Result<KeyMaterial, KeyProviderError> {
        #[cfg(feature = "hsm_pkcs11")]
        {
            return self.get_from_hsm().await;
        }
        
        Err(KeyProviderError::ProviderNotAvailable(
            "HSM/PKCS#11 not available. Compile with feature 'hsm_pkcs11' to enable.".into()
        ))
    }
    
    fn provider_type(&self) -> &'static str {
        "hsm"
    }
    
    fn name(&self) -> &str {
        &self.name
    }
}

impl HsmKeyProvider {
    #[cfg(feature = "hsm_pkcs11")]
    async fn get_from_hsm(&self) -> Result<KeyMaterial, KeyProviderError> {
        use pkcs11::{Context, Session, ObjectHandle, SearchExpression, Attribute, ObjectClass};
        
        let ctx = Context::try_new(&self.library_path)
            .map_err(|e| KeyProviderError::Hsm(format!("failed to load PKCS#11 lib: {}", e)))?;
        
        let slot = self.find_slot(&ctx)?;
        let session = ctx.open_session(slot, CKF_SERIAL_SESSION | CKF_RW_SESSION)
            .map_err(|e| KeyProviderError::Hsm(format!("failed to open session: {}", e)))?;
        
        let key_handle = self.find_key(&session)?;
        let key_bytes = self.export_key(&session, key_handle)?;
        
        ctx.close_session(session);
        
        Ok(KeyMaterial::new(key_bytes, format!("hsm_{}", self.name())))
    }
    
    #[cfg(feature = "hsm_pkcs11")]
    fn find_slot(&self, ctx: &Context) -> Result<Slot, KeyProviderError> {
        let slots = ctx.get_all_slots()
            .map_err(|e| KeyProviderError::Hsm(format!("failed to get slots: {}", e)))?;
        
        for slot in slots {
            if let Ok(info) = ctx.get_slot_info(slot) {
                if self.slot_id == info.slot_description().to_string() {
                    return Ok(slot);
                }
            }
        }
        
        Err(KeyProviderError::Hsm(format!("slot not found: {}", self.slot_id)))
    }
    
    #[cfg(feature = "hsm_pkcs11")]
    fn find_key(&self, session: &Session) -> Result<ObjectHandle, KeyProviderError> {
        let mut search = SearchExpression::new();
        search.add_attribute(Attribute::Label(self.key_label.clone().into()));
        search.add_attribute(Attribute::Class(ObjectClass::SECRET_KEY));
        
        let results = session.find_objects(&search, 1)
            .map_err(|e| KeyProviderError::Hsm(format!("key not found: {}", e)))?;
        
        results.first()
            .cloned()
            .ok_or_else(|| KeyProviderError::Hsm(format!(
                "key with label '{}' not found", self.key_label
            )))
    }
    
    #[cfg(feature = "hsm_pkcs11")]
    fn export_key(&self, session: &Session, key: ObjectHandle) -> Result<[u8; 32], KeyProviderError> {
        let value_attr = session.get_attribute_value(key, &[Attribute::Value])
            .map_err(|e| KeyProviderError::Hsm(format!("failed to get key value: {}", e)))?;
        
        if let Some(Attribute::Value(ref bytes)) = value_attr.first() {
            let bytes: [u8; 32] = bytes.as_slice().try_into()
                .map_err(|_| KeyProviderError::Hsm("invalid key length".into()))?;
            Ok(bytes)
        } else {
            Err(KeyProviderError::Hsm("key is not extractable".into()))
        }
    }
}