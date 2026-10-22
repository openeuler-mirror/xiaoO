use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::os::unix::io::AsRawFd;
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

fn use_sdf_from_config_value(config_value: &toml::Value) -> bool {
    config_value
        .get("vault")
        .and_then(|vault| vault.get("use_sdf"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
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

    use_sdf_from_config_value(&config_value)
}

fn ensure_secrets_dir(secrets_path: &Path) -> Result<()> {
    if let Some(parent) = secrets_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create secrets directory {}", parent.display()))?;
    }
    Ok(())
}

/// Advisory cross-process lock serializing read-modify-write cycles on the
/// secrets store (auto-save / set-secret / delete-secret). The lock file is
/// only ever used as an `flock` handle — its contents are never read or
/// written. Held until dropped; the kernel releases it on process exit, so a
/// crash cannot leave it stuck. Read-only paths (`inject`/`get`/`inspect`)
/// skip the lock: the atomic rename in `save_encrypted_store` guarantees they
/// observe either the old or the new complete file, never a torn one.
struct StoreLock {
    _file: File,
}

impl StoreLock {
    fn acquire(secrets_path: &Path) -> Result<Self> {
        let lock_path = lock_file_path(secrets_path);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("failed to open secrets lock file {}", lock_path.display()))?;
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is owned by `file` and stays open for as long as the
        // lock is held; `flock` only manipulates the lock state of that fd.
        let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if result != 0 {
            return Err(std::io::Error::last_os_error()).with_context(|| {
                format!("failed to acquire secrets lock {}", lock_path.display())
            });
        }
        Ok(Self { _file: file })
    }
}

/// Build a hidden sibling of the secrets store in the same directory, so the
/// resulting path (lock or temp file) always shares the store's filesystem.
fn sibling_path(secrets_path: &Path, suffix: &str) -> PathBuf {
    let file_name = secrets_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| LLM_SECRETS_FILE.to_string());
    secrets_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{file_name}{suffix}"))
}

fn lock_file_path(secrets_path: &Path) -> PathBuf {
    sibling_path(secrets_path, ".lock")
}

pub fn save_llm_secret(config_path: &Path, env_name: &str, secret: &str) -> Result<()> {
    let secrets_path = llm_secrets_path(config_path);
    ensure_secrets_dir(&secrets_path)?;

    let use_sdf = get_use_sdf_from_config(config_path);

    let _guard = StoreLock::acquire(&secrets_path)?;
    let mut store = load_secrets_store(&secrets_path, use_sdf)?;
    store
        .api_keys
        .insert(env_name.to_string(), secret.to_string());

    save_encrypted_store(&secrets_path, &store, use_sdf)
}

pub fn delete_llm_secret(config_path: &Path, env_name: &str) -> Result<bool> {
    let secrets_path = llm_secrets_path(config_path);
    if !secrets_path.exists() {
        return Ok(false);
    }
    let use_sdf = get_use_sdf_from_config(config_path);
    let _guard = StoreLock::acquire(&secrets_path)?;
    let mut store = load_secrets_store(&secrets_path, use_sdf)?;
    let removed =
        store.api_keys.remove(env_name).is_some() | store.tokens.remove(env_name).is_some();
    if !removed {
        return Ok(false);
    }
    if store.api_keys.is_empty() && store.tokens.is_empty() {
        fs::remove_file(&secrets_path)
            .with_context(|| format!("failed to delete secrets file {}", secrets_path.display()))?;
    } else {
        save_encrypted_store(&secrets_path, &store, use_sdf)?;
    }
    Ok(true)
}

/// Persist an env-provided API key on first startup only: if the store
/// already holds an entry for `llm.api_key_env` (saved via `set-secret` or
/// the TUI popup), the env value is ignored so a lingering exported variable
/// cannot silently roll an explicitly rotated key back.
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

    let secrets_path = llm_secrets_path(config_path);
    ensure_secrets_dir(&secrets_path)?;

    let use_sdf = use_sdf_from_config_value(&config_value);

    // The existence check and the write must share one lock section, otherwise
    // a concurrent `set-secret` between them could still be clobbered.
    let _guard = StoreLock::acquire(&secrets_path)?;
    let mut store = load_secrets_store(&secrets_path, use_sdf)?;
    if store.api_keys.contains_key(env_name) || store.tokens.contains_key(env_name) {
        tracing::debug!(
            "secret for env {} already persisted; skipping env auto-save",
            env_name
        );
        return Ok(());
    }
    store.api_keys.insert(env_name.to_string(), api_key);
    save_encrypted_store(&secrets_path, &store, use_sdf)?;

    tracing::info!(
        "auto saved API key from env {} to llm_secrets.json",
        env_name
    );

    Ok(())
}

/// Bootstrap LLM secrets for a startup path: persist a first-run API key from
/// the environment, then load the whole store into the process environment.
///
/// The ordering is load-bearing and must not be inverted: injection
/// overwrites the same env names from the store, so injecting first would
/// mask the env-provided key and permanently block the first-run save.
pub fn init_llm_secrets(config_path: &Path) -> Result<()> {
    auto_save_from_env(config_path).with_context(|| {
        format!(
            "failed to auto-save LLM secrets from {}",
            config_path.display()
        )
    })?;
    inject_llm_secrets_into_env(config_path).with_context(|| {
        format!(
            "failed to initialize LLM secrets from {}",
            config_path.display()
        )
    })
}

fn load_secrets_store(path: &Path, use_sdf: bool) -> Result<SecretsStore> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        // A missing store (never created, or removed by a concurrent
        // `delete-secret` between the caller's check and this read) is an
        // empty store, not an error: lock-free readers rely on this to
        // survive benign races without failing startup.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SecretsStore::default())
        }
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to read secrets file {}", path.display()))
        }
    };

    let decrypted = if use_sdf {
        use vault::sdf::{decrypt_secret, init_sdf_provider, DEFAULT_SDF_LIBRARY_PATH};
        if let Err(e) = init_sdf_provider(DEFAULT_SDF_LIBRARY_PATH) {
            anyhow::bail!("SDF 初始化失败: {}", e);
        }
        decrypt_secret(&bytes).map_err(|e| anyhow::anyhow!("SDF 解密失败: {}", e))?
    } else {
        decrypt_aes_gcm(&bytes)?
    };

    serde_json::from_slice(&decrypted)
        .with_context(|| format!("failed to parse secrets file {}", path.display()))
}

/// Serialize and persist the store atomically: write to a sibling temp file
/// with owner-only permissions, fsync it, then rename over the target. A
/// crash or power loss mid-write can therefore never leave a truncated
/// store behind (a torn file would fail every subsequent startup at decrypt
/// time), and the rename can never downgrade the store file to the umask
/// default. Callers must hold the store lock.
fn save_encrypted_store(path: &Path, store: &SecretsStore, use_sdf: bool) -> Result<()> {
    let json = serde_json::to_vec(store).context("failed to serialize secrets")?;

    let encrypted = if use_sdf {
        use vault::sdf::{encrypt_secret, init_sdf_provider, DEFAULT_SDF_LIBRARY_PATH};
        if let Err(e) = init_sdf_provider(DEFAULT_SDF_LIBRARY_PATH) {
            anyhow::bail!("SDF 初始化失败: {}", e);
        }
        encrypt_secret(json.as_ref()).map_err(|e| anyhow::anyhow!("SDF 加密失败: {}", e))?
    } else {
        encrypt_aes_gcm(&json)?
    };

    let temp_path = temp_file_path(path);
    if let Err(error) = write_secret_file(&temp_path, &encrypted) {
        let _ = fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to write secrets file {}", temp_path.display()));
    }
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error)
            .with_context(|| format!("failed to persist secrets file {}", path.display()));
    }
    Ok(())
}

/// Write `payload` to `path` with owner-only permissions and flush it to
/// stable storage before returning, so the subsequent rename publishes a
/// fully written file and never a umask-default mode. `mode()` only applies
/// when the file is created, hence the unconditional `set_permissions`.
fn write_secret_file(path: &Path, payload: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(payload)?;
    file.sync_all()
}

/// Temp target for the atomic store write; same directory as the store so the
/// final rename stays on one filesystem.
fn temp_file_path(secrets_path: &Path) -> PathBuf {
    sibling_path(secrets_path, &format!(".tmp.{}", std::process::id()))
}

fn encrypt_aes_gcm(plaintext: &[u8]) -> Result<Vec<u8>> {
    use aes_gcm::{
        aead::{Aead, KeyInit},
        Aes256Gcm, Nonce,
    };
    use rand::RngCore;
    use vault::WhiteBoxKeyProvider;

    tracing::warn!(
        "LLM secrets are being encrypted with the whitebox test-grade key \
         (vault.use_sdf=false); configure SDF or HSM for production environments"
    );
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

    tracing::warn!(
        "LLM secrets are being decrypted with the whitebox test-grade key \
         (vault.use_sdf=false); configure SDF or HSM for production environments"
    );
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
#[path = "../../../tests/unit/shared/llm_secrets_test.rs"]
mod tests;
