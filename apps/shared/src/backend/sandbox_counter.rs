use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use once_cell::sync::Lazy;
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub const MAX_ACTIVE_SANDBOXES_PER_KEY: usize = 20;

/// How long a pending reservation is considered valid. Reservations older
/// than this are garbage-collected on load, reclaiming slots leaked by
/// processes that crashed between `check_and_reserve` and
/// `confirm_creation`/`cancel_reservation`.
pub const PENDING_RESERVATION_TTL_MS: u64 = 300_000;

/// Number of PBKDF2-HMAC-SHA256 iterations used to derive the stored form
/// of `key_identifier`. The `key_identifier` for e2b sandboxes is the
/// resolved e2b API key, so it must be one-way hashed before being written
/// to the shared `sandbox_counts.json` / `backend_registry.json` files to
/// avoid persisting the secret in plaintext.
pub const KEY_IDENTIFIER_PBKDF2_ITERATIONS: u32 = 10_000;

/// Fixed application-specific salt for the PBKDF2 derivation. A fixed salt
/// is acceptable here because the derived value is used as a deterministic
/// lookup key (same input → same derived output) rather than as a password
/// hash; the salt prevents cross-application rainbow tables.
const KEY_IDENTIFIER_SALT: &[u8] = b"xiaoo-sandbox-key-v1";

/// Derived output length in bytes (256 bits). Base64-url-no-pad encoding
/// expands this to 43 ASCII characters.
const KEY_IDENTIFIER_DERIVED_LEN: usize = 32;

static IN_PROCESS_LOCK: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

/// One-way hash of a `key_identifier` (which may be a provider API key)
/// using PBKDF2-HMAC-SHA256 with 10 000 iterations and a fixed
/// application-specific salt. The 32-byte derived output is base64url
/// (no padding) encoded so it can be stored as a JSON string key and
/// compared by string equality.
///
/// Because the derivation is deterministic, the same plaintext always
/// produces the same derived string: lookups call `hash_key_identifier`
/// (via `SandboxCounterKey::new`) and compare the derived string against
/// the value persisted in the JSON file.
fn hash_key_identifier(key_identifier: &str) -> String {
    let mut derived = [0u8; KEY_IDENTIFIER_DERIVED_LEN];
    pbkdf2_hmac::<Sha256>(
        key_identifier.as_bytes(),
        KEY_IDENTIFIER_SALT,
        KEY_IDENTIFIER_PBKDF2_ITERATIONS,
        &mut derived,
    );
    URL_SAFE_NO_PAD.encode(derived)
}

#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub struct SandboxCounterKey {
    pub sandbox_type: String,
    /// Derived (PBKDF2-hashed) form of the original key identifier. For
    /// e2b sandboxes the raw identifier is the provider API key, so it is
    /// one-way hashed in [`SandboxCounterKey::new`] before being stored
    /// here — the plaintext is never persisted to disk via serde or
    /// [`SandboxCounterKey::to_string_key`].
    pub key_identifier: String,
}

impl SandboxCounterKey {
    pub fn new(sandbox_type: impl Into<String>, key_identifier: impl Into<String>) -> Self {
        // Always one-way hash the raw identifier here. serde Deserialize
        // bypasses `new` and sets the field directly from the on-disk value
        // (which is already the derived form), so deserialised keys are NOT
        // re-hashed and compare equal to keys built via `new` for the same
        // plaintext. No struct-literal construction of `SandboxCounterKey`
        // exists in the codebase, so every plaintext identifier enters
        // through this single hashing choke point.
        Self {
            sandbox_type: sandbox_type.into(),
            key_identifier: hash_key_identifier(&key_identifier.into()),
        }
    }

    pub fn to_string_key(&self) -> String {
        format!("{}:{}", self.sandbox_type, self.key_identifier)
    }
}

/// A single pending sandbox reservation. Multiple concurrent reservations
/// for the same key are tracked as a list so that stale ones (from crashed
/// processes) can be garbage-collected individually without losing the
/// remaining valid reservations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingReservation {
    pub reserved_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SandboxCounterData {
    pub counts: HashMap<String, usize>,
    #[serde(default)]
    pub pending_reservations: HashMap<String, Vec<PendingReservation>>,
}

#[derive(Debug, Clone)]
pub enum SandboxCounterError {
    LimitExceeded {
        current: usize,
        max: usize,
        key: SandboxCounterKey,
    },
    FileError {
        message: String,
    },
    LockError {
        message: String,
    },
    ParseError {
        message: String,
    },
}

impl std::fmt::Display for SandboxCounterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LimitExceeded { current, max, key } => write!(
                f,
                "limit exceeded for {} {}: current={}, max={}",
                key.sandbox_type, key.key_identifier, current, max
            ),
            Self::FileError { message } => write!(f, "file error: {}", message),
            Self::LockError { message } => write!(f, "lock error: {}", message),
            Self::ParseError { message } => write!(f, "parse error: {}", message),
        }
    }
}

impl std::error::Error for SandboxCounterError {}

#[derive(Debug, Default, Deserialize)]
struct SandboxGlobalConfig {
    #[serde(default)]
    max_sandbox_cnt: Option<usize>,
}

pub fn global_sandbox_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("xiaoo").join("sandbox.toml")
}

pub fn load_max_sandbox_cnt_from_path(path: &Path) -> Option<usize> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                "failed to read global sandbox config; using default max_sandbox_cnt"
            );
            return None;
        }
    };
    match toml::from_str::<SandboxGlobalConfig>(&content) {
        Ok(config) => config.max_sandbox_cnt,
        Err(error) => {
            tracing::warn!(
                error = %error,
                path = %path.display(),
                "failed to parse global sandbox config; using default max_sandbox_cnt"
            );
            None
        }
    }
}

pub fn load_global_max_sandbox_cnt() -> Option<usize> {
    load_max_sandbox_cnt_from_path(&global_sandbox_config_path())
}

pub struct SandboxCounter {
    storage_path: PathBuf,
    lock_path: PathBuf,
    max_per_key: usize,
}

impl SandboxCounter {
    pub fn new() -> Self {
        let max_per_key = load_global_max_sandbox_cnt().unwrap_or(MAX_ACTIVE_SANDBOXES_PER_KEY);
        Self::new_with_limit(max_per_key)
    }

    pub fn new_with_limit(max_per_key: usize) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let xiaoo_dir = home.join(".xiaoo");
        std::fs::create_dir_all(&xiaoo_dir).ok();

        Self {
            storage_path: xiaoo_dir.join("sandbox_counts.json"),
            lock_path: xiaoo_dir.join("sandbox_counts.lock"),
            max_per_key,
        }
    }

    pub fn max_per_key(&self) -> usize {
        self.max_per_key
    }

    pub async fn check_and_reserve(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        let current = data.counts.get(&string_key).copied().unwrap_or(0);
        let pending = data
            .pending_reservations
            .get(&string_key)
            .map(Vec::len)
            .unwrap_or(0);

        if current + pending >= self.max_per_key {
            return Err(SandboxCounterError::LimitExceeded {
                current: current + pending,
                max: self.max_per_key,
                key: key.clone(),
            });
        }

        data.pending_reservations
            .entry(string_key.clone())
            .or_default()
            .push(PendingReservation {
                reserved_at_ms: current_time_ms(),
            });
        self.save_data(&data)?;

        Ok(())
    }

    pub async fn confirm_creation(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        // Pop one pending reservation (if any). If the reservation was
        // already GC'd by the TTL (e.g. a slow build), we still increment
        // the confirmed count so the counter stays accurate.
        if let Some(reservations) = data.pending_reservations.get_mut(&string_key) {
            reservations.pop();
            if reservations.is_empty() {
                data.pending_reservations.remove(&string_key);
            }
        }

        let current = data.counts.get(&string_key).copied().unwrap_or(0);
        data.counts.insert(string_key, current + 1);

        self.save_data(&data)?;
        Ok(())
    }

    pub async fn cancel_reservation(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        if let Some(reservations) = data.pending_reservations.get_mut(&string_key) {
            reservations.pop();
            if reservations.is_empty() {
                data.pending_reservations.remove(&string_key);
            }
            self.save_data(&data)?;
        }

        Ok(())
    }

    pub async fn release(&self, key: &SandboxCounterKey) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        let current = data.counts.get(&string_key).copied().unwrap_or(0);
        if current > 0 {
            data.counts.insert(string_key, current - 1);
            self.save_data(&data)?;
        }

        Ok(())
    }

    /// Reset the confirmed counts to match the supplied live counts, which
    /// are computed by the caller from the shared `BackendRegistry` (only
    /// entries whose owner heartbeat is still fresh). This is the
    /// startup-time reconciliation that purges "zombie" counts left in the
    /// persisted file by previous processes that exited or crashed without
    /// releasing their sandboxes. Pending reservations are left untouched
    /// (the TTL-based GC already reclaims stale ones).
    pub async fn reconcile_counts(
        &self,
        live_counts: HashMap<String, usize>,
    ) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let mut changed = false;
        // Drop keys that no longer have any live backends.
        for key in data.counts.keys().cloned().collect::<Vec<_>>() {
            if !live_counts.contains_key(&key) {
                data.counts.remove(&key);
                changed = true;
            }
        }
        // Overwrite the rest with the freshly computed live counts.
        for (key, count) in &live_counts {
            if data.counts.get(key).copied() != Some(*count) {
                data.counts.insert(key.clone(), *count);
                changed = true;
            }
        }

        if changed {
            self.save_data(&data)?;
        }
        Ok(())
    }

    pub async fn get_count(&self, key: &SandboxCounterKey) -> Result<usize, SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;
        Ok(data.counts.get(&key.to_string_key()).copied().unwrap_or(0))
    }

    pub async fn get_pending_count(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<usize, SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;
        Ok(data
            .pending_reservations
            .get(&key.to_string_key())
            .map(Vec::len)
            .unwrap_or(0))
    }

    pub async fn get_total_count(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<usize, SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;
        let string_key = key.to_string_key();
        let current = data.counts.get(&string_key).copied().unwrap_or(0);
        let pending = data
            .pending_reservations
            .get(&string_key)
            .map(Vec::len)
            .unwrap_or(0);
        Ok(current + pending)
    }

    fn acquire_lock(&self) -> Result<File, SandboxCounterError> {
        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|e| SandboxCounterError::LockError {
                message: format!("failed to create lock file: {}", e),
            })?;

        let fd = lock_file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if result != 0 {
            return Err(SandboxCounterError::LockError {
                message: format!(
                    "failed to acquire lock: {}",
                    std::io::Error::last_os_error()
                ),
            });
        }

        Ok(lock_file)
    }

    fn load_data(&self) -> Result<SandboxCounterData, SandboxCounterError> {
        if !self.storage_path.exists() {
            return Ok(SandboxCounterData::default());
        }

        let file = File::open(&self.storage_path).map_err(|e| SandboxCounterError::FileError {
            message: format!("failed to open storage file: {}", e),
        })?;

        let reader = BufReader::new(file);
        let mut data: SandboxCounterData =
            serde_json::from_reader(reader).map_err(|e| SandboxCounterError::ParseError {
                message: format!("failed to parse storage file: {}", e),
            })?;

        let removed = self.gc_stale_pending(&mut data);
        if removed {
            // Persist the cleaned state so subsequent readers don't reprocess
            // the same stale entries.
            let _ = self.save_data(&data);
        }

        Ok(data)
    }

    /// Remove pending reservations older than `PENDING_RESERVATION_TTL_MS`.
    /// Returns true if any entries were removed.
    fn gc_stale_pending(&self, data: &mut SandboxCounterData) -> bool {
        let now = current_time_ms();
        let mut removed = false;
        for reservations in data.pending_reservations.values_mut() {
            let before = reservations.len();
            reservations
                .retain(|r| now.saturating_sub(r.reserved_at_ms) < PENDING_RESERVATION_TTL_MS);
            if reservations.len() != before {
                removed = true;
            }
        }
        data.pending_reservations.retain(|_, v| {
            if v.is_empty() {
                removed = true;
                false
            } else {
                true
            }
        });
        removed
    }

    fn save_data(&self, data: &SandboxCounterData) -> Result<(), SandboxCounterError> {
        let file =
            File::create(&self.storage_path).map_err(|e| SandboxCounterError::FileError {
                message: format!("failed to create storage file: {}", e),
            })?;

        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, data).map_err(|e| SandboxCounterError::ParseError {
            message: format!("failed to write storage file: {}", e),
        })?;

        writer.flush().map_err(|e| SandboxCounterError::FileError {
            message: format!("failed to flush storage file: {}", e),
        })?;

        Ok(())
    }
}

impl Default for SandboxCounter {
    fn default() -> Self {
        Self::new()
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_identifier_is_hashed_and_deterministic() {
        // The raw identifier must NOT appear in the stored form.
        let key = SandboxCounterKey::new("e2b", "secret-api-key-12345");
        assert_ne!(
            key.key_identifier, "secret-api-key-12345",
            "key_identifier must be hashed, not stored as plaintext"
        );
        // Same plaintext → same derived form (deterministic lookup).
        let again = SandboxCounterKey::new("e2b", "secret-api-key-12345");
        assert_eq!(key.key_identifier, again.key_identifier);
        // Different plaintext → different derived form.
        let other = SandboxCounterKey::new("e2b", "different-api-key-67890");
        assert_ne!(key.key_identifier, other.key_identifier);
        // The derived form is 43 chars (32 bytes base64url-no-pad).
        assert_eq!(key.key_identifier.len(), 43);
        // The plaintext must not leak through `to_string_key` either.
        assert!(!key.to_string_key().contains("secret-api-key-12345"));
    }

    #[tokio::test]
    async fn test_check_and_reserve_at_limit() {
        let counter = SandboxCounter::new_with_limit(MAX_ACTIVE_SANDBOXES_PER_KEY);
        let key = SandboxCounterKey::new("e2b", "test-limit");

        for _ in 0..MAX_ACTIVE_SANDBOXES_PER_KEY {
            counter
                .check_and_reserve(&key)
                .await
                .expect("should reserve");
            counter
                .confirm_creation(&key)
                .await
                .expect("should confirm");
        }

        let result = counter.check_and_reserve(&key).await;
        assert!(matches!(
            result,
            Err(SandboxCounterError::LimitExceeded { .. })
        ));

        for _ in 0..MAX_ACTIVE_SANDBOXES_PER_KEY {
            counter.release(&key).await.expect("should release");
        }
    }

    #[tokio::test]
    async fn test_confirm_and_release() {
        let counter = SandboxCounter::new_with_limit(MAX_ACTIVE_SANDBOXES_PER_KEY);
        let key = SandboxCounterKey::new("e2b", "test-confirm");

        counter
            .check_and_reserve(&key)
            .await
            .expect("should reserve");
        counter
            .confirm_creation(&key)
            .await
            .expect("should confirm");

        let count = counter.get_count(&key).await.expect("should get count");
        assert_eq!(count, 1);

        counter.release(&key).await.expect("should release");
        let count = counter.get_count(&key).await.expect("should get count");
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_pending_counts() {
        let counter = SandboxCounter::new_with_limit(MAX_ACTIVE_SANDBOXES_PER_KEY);
        let key = SandboxCounterKey::new("e2b", "test-pending");

        counter
            .check_and_reserve(&key)
            .await
            .expect("should reserve");

        let pending = counter
            .get_pending_count(&key)
            .await
            .expect("should get pending");
        assert_eq!(pending, 1);

        let total = counter
            .get_total_count(&key)
            .await
            .expect("should get total");
        assert_eq!(total, 1);

        counter
            .cancel_reservation(&key)
            .await
            .expect("should cancel");

        let pending = counter
            .get_pending_count(&key)
            .await
            .expect("should get pending");
        assert_eq!(pending, 0);
    }

    #[tokio::test]
    async fn test_in_process_lock_protection() {
        let counter = Arc::new(SandboxCounter::new_with_limit(MAX_ACTIVE_SANDBOXES_PER_KEY));
        let key = SandboxCounterKey::new("e2b", "test-concurrent");

        let mut tasks = Vec::new();
        for _ in 0..5 {
            let counter_clone = counter.clone();
            let key_clone = key.clone();
            let task =
                tokio::spawn(async move { counter_clone.check_and_reserve(&key_clone).await });
            tasks.push(task);
        }

        let results: Vec<_> = futures::future::join_all(tasks).await;

        let success_count = results
            .iter()
            .filter(|r| r.is_ok() && r.as_ref().unwrap().is_ok())
            .count();
        assert!(success_count <= MAX_ACTIVE_SANDBOXES_PER_KEY);

        for _ in 0..success_count {
            counter
                .cancel_reservation(&key)
                .await
                .expect("should cancel");
        }
    }

    #[tokio::test]
    async fn test_stale_pending_reservations_are_gc_on_load() {
        let counter = SandboxCounter::new_with_limit(MAX_ACTIVE_SANDBOXES_PER_KEY);
        let key = SandboxCounterKey::new("e2b", "test-stale-gc");

        // Seed a stale pending reservation directly into the data file.
        {
            let _in_process = IN_PROCESS_LOCK.lock().await;
            let stale_ms = current_time_ms().saturating_sub(PENDING_RESERVATION_TTL_MS + 1_000);
            let data = SandboxCounterData {
                counts: HashMap::new(),
                pending_reservations: HashMap::from([(
                    key.to_string_key(),
                    vec![
                        PendingReservation {
                            reserved_at_ms: stale_ms,
                        },
                        PendingReservation {
                            reserved_at_ms: stale_ms,
                        },
                    ],
                )]),
            };
            counter.save_data(&data).expect("save stale data");
        }

        // The stale reservations should be GC'd on load, so the pending
        // count reports 0 and a fresh reservation succeeds.
        let pending = counter
            .get_pending_count(&key)
            .await
            .expect("should get pending after gc");
        assert_eq!(pending, 0, "stale reservations should have been gc'd");

        counter
            .check_and_reserve(&key)
            .await
            .expect("should reserve after gc freed the slot");
        counter
            .cancel_reservation(&key)
            .await
            .expect("should cancel");
    }

    #[tokio::test]
    async fn test_fresh_pending_reservations_survive_gc() {
        let counter = SandboxCounter::new_with_limit(MAX_ACTIVE_SANDBOXES_PER_KEY);
        let key = SandboxCounterKey::new("e2b", "test-fresh-pending");

        counter
            .check_and_reserve(&key)
            .await
            .expect("should reserve");

        let pending = counter
            .get_pending_count(&key)
            .await
            .expect("should get pending");
        assert_eq!(pending, 1, "fresh reservation should survive gc");

        counter
            .cancel_reservation(&key)
            .await
            .expect("should cancel");
    }

    #[test]
    fn load_max_sandbox_cnt_from_path_parses_value() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("sandbox.toml");
        std::fs::write(&path, "max_sandbox_cnt = 7\n").expect("write config");

        assert_eq!(load_max_sandbox_cnt_from_path(&path), Some(7));
    }

    #[test]
    fn load_max_sandbox_cnt_from_path_missing_file_returns_none() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("does-not-exist.toml");

        assert_eq!(load_max_sandbox_cnt_from_path(&path), None);
    }

    #[test]
    fn load_max_sandbox_cnt_from_path_absent_field_returns_none() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("sandbox.toml");
        std::fs::write(&path, "[other]\nkey = 1\n").expect("write config");

        assert_eq!(load_max_sandbox_cnt_from_path(&path), None);
    }

    #[test]
    fn load_max_sandbox_cnt_from_path_invalid_toml_returns_none() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("sandbox.toml");
        std::fs::write(&path, "not = valid = toml\n").expect("write config");

        assert_eq!(load_max_sandbox_cnt_from_path(&path), None);
    }
}
