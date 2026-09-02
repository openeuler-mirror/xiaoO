use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

const LLM_SECRETS_FILE: &str = "llm_secrets.json";

#[derive(Default, serde::Serialize, serde::Deserialize)]
struct SecretsStore {
    #[serde(default)]
    api_keys: BTreeMap<String, String>,
    #[serde(default)]
    tokens: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretStoreMetadata {
    pub api_keys: Vec<String>,
    pub tokens: Vec<String>,
}

fn get_use_sdf_from_config(config_path: &Path) -> bool {
    let config_content = match fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let config_value: toml::Value = match config_content.parse() {
        Ok(v) => v,
        Err(_) => return false,
    };

    config_value
        .get("vault")
        .and_then(|vault| vault.get("use_sdf"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()> {
    let secrets_path = llm_secrets_path(config_path);

    if let Some(parent) = secrets_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create secrets directory {}", parent.display()))?;
    }

    let use_sdf = get_use_sdf_from_config(config_path);

    let mut store = load_secrets_store(&secrets_path, use_sdf)?;
    store
        .api_keys
        .insert(env_name.to_string(), secret.to_string());

    save_encrypted_store(&secrets_path, &store, use_sdf)
}

pub fn auto_save_from_env(config_path: &Path) -> Result<()> {
    let config_content = match fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => return Ok(()),
    };

    let config_value: toml::Value = match config_content.parse() {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    let api_key_env = config_value
        .get("llm")
        .and_then(|llm| llm.get("api_key_env"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty());

    let Some(env_name) = api_key_env else {
        return Ok(());
    };

    let api_key = match std::env::var(env_name) {
        Ok(v) if !v.trim().is_empty() => v,
        Ok(_) => return Ok(()),
        Err(_) => return Ok(()),
    };

    save_llm_secret(config_path, env_name, &api_key)?;

    tracing::info!(
        "auto saving API key from env {} to llm_secrets.json",
        env_name
    );

    Ok(())
}

fn load_secrets_store(path: &Path, use_sdf: bool) -> Result<SecretsStore> {
    if !path.exists() {
        return Ok(SecretsStore::default());
    }

    let bytes = fs::read(path)
        .with_context(|| format!("failed to read secrets file {}", path.display()))?;

    let decrypted = if use_sdf {
        use vault::sdf::{decrypt_secret, init_sdf_provider};
        if let Err(e) = init_sdf_provider("/usr/local/sdf/lib/libsdf.so") {
            anyhow::bail!("SDF 初始化失败: {}", e);
        }
        decrypt_secret(&bytes).map_err(|e| anyhow::anyhow!("SDF 解密失败: {}", e))?
    } else {
        decrypt_aes_gcm(&bytes)?
    };

    serde_json::from_slice(&decrypted)
        .with_context(|| format!("failed to parse secrets file {}", path.display()))
}

fn save_encrypted_store(path: &Path, store: &SecretsStore, use_sdf: bool) -> Result<()> {
    let json = serde_json::to_vec(store).context("failed to serialize secrets")?;

    let encrypted = if use_sdf {
        use vault::sdf::{encrypt_secret, init_sdf_provider};
        if let Err(e) = init_sdf_provider("/usr/local/sdf/lib/libsdf.so") {
            anyhow::bail!("SDF 初始化失败: {}", e);
        }
        encrypt_secret(json.as_ref()).map_err(|e| anyhow::anyhow!("SDF 加密失败: {}", e))?
    } else {
        encrypt_aes_gcm(&json)?
    };

    fs::write(path, encrypted)
        .with_context(|| format!("failed to write secrets file {}", path.display()))?;

    Ok(())
}

fn encrypt_aes_gcm(plaintext: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    use rand::RngCore;
    use vault::WhiteBoxKeyProvider;

    let key_provider = WhiteBoxKeyProvider::new("default");
    let master_key = key_provider
        .get_key()
        .context("failed to get key material")?;

    let cipher = Aes256Gcm::new_from_slice(&master_key)
        .map_err(|_| anyhow::anyhow!("invalid master key"))?;

    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|_| anyhow::anyhow!("encryption failed"))?;

    let mut result = vec![1u8];
    result.extend(&nonce_bytes);
    result.extend(&ciphertext);
    Ok(result)
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
    let master_key = key_provider
        .get_key()
        .context("failed to get key material")?;

    let cipher = Aes256Gcm::new_from_slice(&master_key)
        .map_err(|_| anyhow::anyhow!("invalid master key"))?;

    let nonce = Nonce::from_slice(&encrypted[1..13]);
    let ciphertext = &encrypted[13..];

    cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| anyhow::anyhow!("decryption failed"))
}

pub fn llm_secrets_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(LLM_SECRETS_FILE)
}

/// Resolve one LLM secret for a specific configuration without changing the
/// process environment used by a running daemon.
pub fn get_llm_secret(config_path: &Path, env_name: &str) -> Result<String> {
    let store = load_secrets_store(
        &llm_secrets_path(config_path),
        get_use_sdf_from_config(config_path),
    )?;
    store
        .api_keys
        .get(env_name)
        .or_else(|| store.tokens.get(env_name))
        .cloned()
        .or_else(|| std::env::var(env_name).ok())
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("API key environment variable {env_name} not found"))
}

/// Load the selected configuration's encrypted secrets into the process
/// environment before runtime assembly.
pub fn inject_llm_secrets_into_env(config_path: &Path) -> Result<()> {
    let store = load_secrets_store(
        &llm_secrets_path(config_path),
        get_use_sdf_from_config(config_path),
    )?;
    for (environment, secret) in store.api_keys.into_iter().chain(store.tokens) {
        std::env::set_var(environment, secret);
    }
    Ok(())
}

/// Inspect the encrypted store without exposing any secret values.
pub fn inspect_secret_store(config_path: &Path) -> Result<SecretStoreMetadata> {
    let store = load_secrets_store(
        &llm_secrets_path(config_path),
        get_use_sdf_from_config(config_path),
    )?;
    Ok(SecretStoreMetadata {
        api_keys: store.api_keys.into_keys().collect(),
        tokens: store.tokens.into_keys().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        get_llm_secret, inject_llm_secrets_into_env, inspect_secret_store, save_llm_secret,
    };

    #[test]
    fn saved_secret_can_be_retrieved_on_demand() {
        let temp_dir = tempfile::TempDir::new().expect("create temp dir");
        let config_path = temp_dir.path().join("config.toml");
        let env_name = "XIAOO_TEST_DEEPSEEK_API_KEY";

        std::env::remove_var(env_name);
        std::env::set_var("USE_SDF", "false");
        save_llm_secret(&config_path, env_name, "test-secret").expect("save secret");

        assert_eq!(
            get_llm_secret(&config_path, env_name).expect("resolve selected config secret"),
            "test-secret"
        );
        inject_llm_secrets_into_env(&config_path).expect("inject secret");
        assert_eq!(std::env::var(env_name).as_deref(), Ok("test-secret"));

        let metadata = inspect_secret_store(&config_path).expect("inspect secret store");
        assert_eq!(metadata.api_keys, vec![env_name.to_string()]);
        assert!(metadata.tokens.is_empty());

        std::env::remove_var(env_name);
        std::env::remove_var("USE_SDF");
    }
}
