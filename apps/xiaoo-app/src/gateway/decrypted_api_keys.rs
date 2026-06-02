use std::path::PathBuf;
use std::sync::OnceLock;
use anyhow::{bail, Context, Result};

static SECRET_PROVIDER: OnceLock<SecretProvider> = OnceLock::new();

pub struct SecretProvider {
    secrets_path: PathBuf,
    use_sdf: bool,
}

impl SecretProvider {
    pub fn new(secrets_path: PathBuf, use_sdf: bool) -> Self {
        Self {
            secrets_path,
            use_sdf,
        }
    }

    pub fn init(secrets_path: PathBuf, use_sdf: bool) {
        let provider = SecretProvider::new(secrets_path, use_sdf);
        SECRET_PROVIDER.set(provider).ok();
    }

    pub fn get_secret(&self, env_name: &str) -> Result<String> {
        if !self.secrets_path.exists() {
            return Ok(self.load_from_env(env_name)?);
        }

        let bytes = std::fs::read(&self.secrets_path)
            .context("failed to read secrets file")?;

        if bytes.is_empty() {
            return Ok(self.load_from_env(env_name)?);
        }

        if bytes.len() < 2 {
            std::fs::remove_file(&self.secrets_path).ok();
            return Ok(self.load_from_env(env_name)?);
        }

        let decrypted = if self.use_sdf {
            use vault::sdf::{init_sdf_provider, decrypt_secret};
            init_sdf_provider("/usr/local/sdf/lib/libsdf.so")?;
            decrypt_secret(&bytes)?
        } else {
            decrypt_aes_gcm(&bytes)?
        };

        let store: SecretsStore = serde_json::from_slice(&decrypted)
            .context("failed to parse secrets file")?;

        if let Some(secret) = store.api_keys.get(env_name) {
            return Ok(secret.clone());
        }

        if let Some(secret) = store.tokens.get(env_name) {
            return Ok(secret.clone());
        }

        self.load_from_env(env_name)
    }

    fn load_from_env(&self, env_name: &str) -> Result<String> {
        std::env::var(env_name)
            .map_err(|_| anyhow::anyhow!("API key environment variable {} not found", env_name))
    }

    pub fn get_secrets_path(&self) -> &PathBuf {
        &self.secrets_path
    }

    pub fn is_use_sdf(&self) -> bool {
        self.use_sdf
    }
}

#[derive(serde::Deserialize)]
struct SecretsStore {
    #[serde(default)]
    api_keys: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    tokens: std::collections::BTreeMap<String, String>,
}

fn decrypt_aes_gcm(encrypted: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    use vault::WhiteBoxKeyProvider;

    if encrypted.len() < 13 {
        bail!("encrypted data too short");
    }

    let version = encrypted[0];
    if version != 1 {
        bail!("unsupported encryption version: {}", version);
    }

    let key_provider = WhiteBoxKeyProvider::new("default");
    let master_key = key_provider.get_key()
        .context("failed to get key material")?;

    let cipher = Aes256Gcm::new_from_slice(&master_key)
        .map_err(|_| anyhow::anyhow!("invalid master key"))?;

    let nonce = Nonce::from_slice(&encrypted[1..13]);
    let ciphertext = &encrypted[13..];

    cipher.decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed"))
}

pub fn get_decrypted_api_key(env_name: &str) -> Option<String> {
    SECRET_PROVIDER
        .get()
        .and_then(|p| p.get_secret(env_name).ok())
}

pub fn init_secret_provider(secrets_path: PathBuf, use_sdf: bool) {
    SecretProvider::init(secrets_path, use_sdf);
}
