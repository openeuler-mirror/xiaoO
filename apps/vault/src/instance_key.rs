//! Per-instance master key provider.
//!
//! A random 32-byte key (OS CSPRNG) is created on first use and persisted
//! next to the secrets store with owner-only permissions, so the source
//! code alone cannot decrypt a store.

use crate::key_provider_error::KeyProviderError;
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::Sha256;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;
use zeroize::Zeroizing;

type HmacSha256 = Hmac<Sha256>;
type MasterKey = Zeroizing<[u8; KEY_LEN]>;

const KEY_LEN: usize = 32;
const TAG_LEN: usize = 16;
const KEY_CHECK_LEN: usize = 32;

const MASTER_KEY_LABEL: &[u8] = b"xiaoo-instance-master-key-v1";
const STORE_V2_LABEL: &[u8] = b"xiaoo-llm-secrets-store-v2";

/// The winner of a lost `O_EXCL` race publishes its bytes only after write +
/// fsync, so the reload tolerates transient reads; bounded to fail fast on
/// real corruption.
const CREATE_RACE_RELOAD_ATTEMPTS: usize = 8;
const CREATE_RACE_RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// Master-key provider backed by a per-instance random key file.
pub struct InstanceKeyProvider {
    key_path: PathBuf,
}

impl InstanceKeyProvider {
    pub fn new(key_path: impl Into<PathBuf>) -> Self {
        Self {
            key_path: key_path.into(),
        }
    }

    /// Load the master key, creating it on first use.
    pub fn load_or_create(&self) -> Result<MasterKey, KeyProviderError> {
        match load_verified(&self.key_path) {
            Ok(key) => Ok(key),
            Err(error) if is_recoverable(&error, &self.key_path) => {
                if error.kind() == std::io::ErrorKind::InvalidData {
                    if let Err(error) = fs::remove_file(&self.key_path) {
                        if error.kind() != std::io::ErrorKind::NotFound {
                            return Err(error.into());
                        }
                    }
                }
                self.create()
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Load the master key without creating it.
    pub fn load(&self) -> Result<MasterKey, KeyProviderError> {
        load_verified(&self.key_path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                KeyProviderError::KeyNotFound(format!(
                    "master key file {} not found; restore it from backup, or remove the \
                     secrets store and re-enter credentials",
                    self.key_path.display()
                ))
            } else {
                error.into()
            }
        })
    }

    fn create(&self) -> Result<MasterKey, KeyProviderError> {
        let key = random_key();
        match self.create_exclusive(&key) {
            Ok(()) => Ok(key),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                reload_after_lost_create_race(&self.key_path).map_err(Into::into)
            }
            Err(error) => Err(error.into()),
        }
    }

    fn create_exclusive(&self, key: &[u8; KEY_LEN]) -> std::io::Result<()> {
        if let Some(parent) = self.key_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.key_path)?;
        file.write_all(&key_file_bytes(key))?;
        file.sync_all()?;
        sync_parent_dir(&self.key_path)
    }
}

/// Reload the key after losing the `O_EXCL` create race: the winner's file
/// is visible before its bytes are complete, so transient reads are retried
/// briefly before the definitive error is returned.
fn reload_after_lost_create_race(key_path: &Path) -> std::io::Result<MasterKey> {
    for attempt in 0..CREATE_RACE_RELOAD_ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(CREATE_RACE_RETRY_INTERVAL);
        }
        match load_verified(key_path) {
            Ok(key) => return Ok(key),
            Err(error) if is_create_race_transient(&error) => continue,
            Err(error) => return Err(error),
        }
    }
    load_verified(key_path)
}

/// Read outcomes that can stem from the winner still writing the key file.
/// `NotFound` is tolerated defensively; nothing deletes the key file.
fn is_create_race_transient(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::NotFound
    )
}

/// A missing key file, or an empty one left by a creation that crashed
/// between `open` and `write`, can be (re)created: stores are only written
/// after the key file is complete, so nothing can reference it.
fn is_recoverable(error: &std::io::Error, key_path: &Path) -> bool {
    if error.kind() == std::io::ErrorKind::NotFound {
        return true;
    }
    error.kind() == std::io::ErrorKind::InvalidData
        && fs::metadata(key_path).is_ok_and(|metadata| metadata.len() == 0)
}

/// Key-confirmation tag bound into the v2 secrets store header.
pub fn store_key_tag(key: &[u8; KEY_LEN]) -> [u8; TAG_LEN] {
    hmac_tag(key, STORE_V2_LABEL)
}

fn random_key() -> MasterKey {
    let mut key = MasterKey::new([0u8; KEY_LEN]);
    rand::thread_rng().fill_bytes(&mut *key);
    key
}

fn hmac_tag(key: &[u8; KEY_LEN], label: &[u8]) -> [u8; TAG_LEN] {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(label);
    let digest = mac.finalize().into_bytes();
    let mut tag = [0u8; TAG_LEN];
    tag.copy_from_slice(&digest[..TAG_LEN]);
    tag
}

fn key_file_bytes(key: &[u8; KEY_LEN]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(KEY_LEN + KEY_CHECK_LEN);
    bytes.extend_from_slice(key);
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    mac.update(MASTER_KEY_LABEL);
    bytes.extend_from_slice(&mac.finalize().into_bytes());
    bytes
}

/// fsync the parent directory so the key's directory entry survives power
/// loss; otherwise the key could vanish while a later v2 store survives.
fn sync_parent_dir(path: &Path) -> std::io::Result<()> {
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    };
    fs::File::open(parent)?.sync_all()
}

fn load_verified(path: &Path) -> std::io::Result<MasterKey> {
    let bytes = fs::read(path)?;
    parse_verified(&bytes).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "master key file {} is corrupted (expected {} bytes: key + hmac tag); \
                 restore it from backup",
                path.display(),
                KEY_LEN + KEY_CHECK_LEN
            ),
        )
    })
}

fn parse_verified(bytes: &[u8]) -> Option<MasterKey> {
    if bytes.len() != KEY_LEN + KEY_CHECK_LEN {
        return None;
    }
    let key = MasterKey::new(bytes[..KEY_LEN].try_into().ok()?);
    let tag: &[u8; KEY_CHECK_LEN] = bytes[KEY_LEN..].try_into().ok()?;
    let mut mac = HmacSha256::new_from_slice(&*key).expect("hmac accepts any key length");
    mac.update(MASTER_KEY_LABEL);
    mac.verify_slice(tag).ok()?;
    Some(key)
}
