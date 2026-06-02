//! Secrets Manager
//!
//! 统一管理 API Key 和 Verification Token
//! 本地加密存储到 llm_secrets.json
//!   - use_sdf=true: 调用 SDF 国密接口加密/解密
//!   - use_sdf=false: 使用 WhiteBox 密钥 + AES-GCM 加密/解密
//!
//! 优先级:
//! - API Key: 环境变量 > 本地存储
//! - Verification Token: config.toml > 本地存储

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use vault::{KeyMaterial, KeyProvider, KeyProviderConfig, WhiteBoxKeyProvider, TeeKeyProvider, TeeType, HsmKeyProvider, encrypt_secret, decrypt_secret};

const LLM_SECRETS_FILE: &str = "llm_secrets.json";

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SecretsStore {
    #[serde(default)]
    pub api_keys: BTreeMap<String, String>,
    #[serde(default)]
    pub verification_token: Option<String>,
}

pub struct KeyProviderFactory;

impl KeyProviderFactory {
    pub fn create(config: &KeyProviderConfig) -> Result<Arc<dyn KeyProvider>> {
        match config {
            KeyProviderConfig::WhiteBox { name } => {
                Ok(Arc::new(WhiteBoxKeyProvider::new(name)))
            }
            KeyProviderConfig::Tee { name, tee_type, slot } => {
                let tee_type = TeeType::from_str(tee_type);
                let provider = TeeKeyProvider::new(name, tee_type);
                let provider = if let Some(slot) = slot {
                    provider.with_slot(slot)
                } else {
                    provider
                };
                Ok(Arc::new(provider))
            }
            KeyProviderConfig::Hsm { name, library_path, slot_id, key_label } => {
                Ok(Arc::new(HsmKeyProvider::new(
                    name,
                    library_path,
                    slot_id,
                    key_label,
                )))
            }
        }
    }
}

pub struct SecretsManager {
    config_path: PathBuf,
    store: Arc<RwLock<SecretsStore>>,
    master_key: Option<[u8; 32]>,
    key_provider: Arc<dyn KeyProvider>,
    use_sdf: bool,
}

impl SecretsManager {
    pub async fn new(
        config_path: PathBuf,
        key_provider_config: KeyProviderConfig,
        use_sdf: bool,
    ) -> Result<Self> {
        let key_provider = KeyProviderFactory::create(&key_provider_config)?;
        let master_key = key_provider.provide().await
            .map(|km| km.key_bytes)
            .ok();

        let store = Arc::new(RwLock::new(SecretsStore::default()));
        let mut manager = Self {
            config_path,
            store,
            master_key,
            key_provider,
            use_sdf,
        };

        manager.load_secrets().await?;
        Ok(manager)
    }

    fn llm_secrets_path(&self) -> PathBuf {
        self.config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(LLM_SECRETS_FILE)
    }

    pub async fn save_llm_secret(&self, env_name: &str, secret: &str) -> Result<()> {
        {
            let mut store = self.store.write().await;
            store.api_keys.insert(env_name.to_string(), secret.to_string());
        }
        self.persist_secrets().await
    }

    pub async fn save_verification_token(&self, token: &str) -> Result<()> {
        {
            let mut store = self.store.write().await;
            store.verification_token = Some(token.to_string());
        }
        self.persist_secrets().await
    }

    async fn persist_secrets(&self) -> Result<()> {
        let store = self.store.read().await;
        let json = serde_json::to_vec_pretty(&*store)
            .context("failed to serialize secrets")?;

        let secrets_path = self.llm_secrets_path();
        if let Some(parent) = secrets_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create secrets directory {}", parent.display()))?;
        }

        let encrypted = self.encrypt_data(&json)?;
        fs::write(&secrets_path, encrypted)
            .with_context(|| format!("failed to write encrypted secrets file {}", secrets_path.display()))?;

        Ok(())
    }

    fn encrypt_data(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        if self.use_sdf {
            encrypt_secret(plaintext).map_err(|e| anyhow::anyhow!("SDF encrypt failed: {}", e))
        } else {
            self.encrypt_data_aes(plaintext)
        }
    }

    fn encrypt_data_aes(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };
        use rand::RngCore;

        let key = self.master_key
            .context("master key not loaded")?;

        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("invalid master key"))?;

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, plaintext)
            .map_err(|_| anyhow::anyhow!("encryption failed"))?;

        let mut result = vec![1u8];
        result.extend(&nonce_bytes);
        result.extend(ciphertext);

        Ok(result)
    }

    fn decrypt_data(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        if self.use_sdf {
            decrypt_secret(encrypted).map_err(|e| anyhow::anyhow!("SDF decrypt failed: {}", e))
        } else {
            self.decrypt_data_aes(encrypted)
        }
    }

    fn decrypt_data_aes(&self, encrypted: &[u8]) -> Result<Vec<u8>> {
        use aes_gcm::{
            aead::{Aead, KeyInit},
            Aes256Gcm, Nonce,
        };

        if encrypted.len() < 13 {
            bail!("encrypted data too short");
        }

        let version = encrypted[0];
        if version != 1 {
            bail!("unsupported encryption version: {}", version);
        }

        let key = self.master_key
            .context("master key not loaded")?;

        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|_| anyhow::anyhow!("invalid master key"))?;

        let nonce = Nonce::from_slice(&encrypted[1..13]);
        let ciphertext = &encrypted[13..];

        cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow::anyhow!("decryption failed"))
    }

    pub async fn load_secrets(&self) -> Result<()> {
        let secrets_path = self.llm_secrets_path();
        let store = self.load_local_encrypted(&secrets_path).await?;
        *self.store.write().await = store;
        Ok(())
    }

    async fn load_local_encrypted(&self, path: &Path) -> Result<SecretsStore> {
        if !path.exists() {
            return Ok(SecretsStore::default());
        }
        let encrypted = fs::read(path)
            .with_context(|| format!("failed to read encrypted secrets file {}", path.display()))?;
        let decrypted = self.decrypt_data(&encrypted)?;
        serde_json::from_slice(&decrypted)
            .with_context(|| format!("failed to parse secrets file {}", path.display()))
    }

    pub async fn resolve_api_key(&self, api_key_env: Option<&str>) -> Result<String> {
        if let Some(env_name) = api_key_env {
            if let Ok(value) = std::env::var(env_name) {
                if !value.trim().is_empty() {
                    tracing::debug!("using API key from environment variable: {}", env_name);
                    return Ok(value);
                }
            }
        }

        let store = self.store.read().await;
        if let Some(env_name) = api_key_env {
            if let Some(key) = store.api_keys.get(env_name) {
                tracing::debug!("using API key from local storage: {}", env_name);
                return Ok(key.clone());
            }
        }

        let env_name_display = api_key_env.unwrap_or("<not configured>");
        bail!(
            "API key not found: environment variable '{}' is not set, and no key found in local storage. \
             Please either:\n  1. Set the environment variable '{}', or\n  2. Configure the API key in TUI",
            env_name_display,
            env_name_display
        );
    }

    pub async fn resolve_verification_token(
        &self,
        config_token: Option<&str>,
    ) -> Result<String> {
        if let Some(token) = config_token {
            let trimmed = token.trim();
            if !trimmed.is_empty() {
                tracing::debug!("using verification_token from config.toml");
                return Ok(trimmed.to_string());
            }
        }

        let store = self.store.read().await;
        if let Some(token) = &store.verification_token {
            tracing::debug!("using verification_token from local storage");
            return Ok(token.clone());
        }

        bail!(
            "verification_token not found: not configured in config.toml, and no token found in local storage. \
             Please either:\n  1. Set 'verification_token' in [channels.feishu] section of config.toml, or\n  2. Store the token via TUI or CLI"
        );
    }
}
