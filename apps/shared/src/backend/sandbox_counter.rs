use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use once_cell::sync::Lazy;
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

pub(crate) const MAX_ACTIVE_SANDBOXES_PER_KEY: usize = 20;

/// How long a pending reservation is considered valid. Reservations older
/// than this are garbage-collected on load, reclaiming slots leaked by
/// processes that crashed between `check_and_reserve` and
/// `confirm_creation`/`cancel_reservation`.
pub(crate) const PENDING_RESERVATION_TTL_MS: u64 = 300_000;

/// Number of PBKDF2-HMAC-SHA256 iterations used to derive the stored form
/// of `key_identifier`. The `key_identifier` for e2b sandboxes is the
/// resolved e2b API key, so it must be one-way hashed before being written
/// to the shared `sandbox_counts.json` / `backend_registry.json` files to
/// avoid persisting the secret in plaintext.
const KEY_IDENTIFIER_PBKDF2_ITERATIONS: u32 = 10_000;

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
pub(crate) struct SandboxCounterKey {
    pub sandbox_type: String,
    /// Derived (PBKDF2-hashed) form of the original key identifier. For
    /// e2b sandboxes the raw identifier is the provider API key, so it is
    /// one-way hashed in [`SandboxCounterKey::new`] before being stored
    /// here — the plaintext is never persisted to disk via serde or
    /// [`SandboxCounterKey::to_string_key`].
    pub key_identifier: String,
}

impl SandboxCounterKey {
    pub(crate) fn new(sandbox_type: impl Into<String>, key_identifier: impl Into<String>) -> Self {
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

    pub(crate) fn to_string_key(&self) -> String {
        format!("{}:{}", self.sandbox_type, self.key_identifier)
    }
}

/// A single pending sandbox reservation. Multiple concurrent reservations
/// for the same key are tracked as a list so that stale ones (from crashed
/// processes) can be garbage-collected individually without losing the
/// remaining valid reservations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct PendingReservation {
    pub reserved_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct SandboxCounterData {
    pub counts: HashMap<String, usize>,
    #[serde(default)]
    pub pending_reservations: HashMap<String, Vec<PendingReservation>>,
    /// Per-key count of "ghost" sandboxes: real sandboxes whose
    /// `confirm_creation` arrived after the pool had already been refilled
    /// to `max_per_key` (their TTL reservation was reclaimed mid-build and
    /// the slot reused). Kept separate from `counts` to preserve the
    /// `counts <= max_per_key` invariant: the over-limit sandbox is
    /// accounted here, and `release` consumes a ghost credit before
    /// decrementing `counts`, so `counts` never dips below the real live
    /// count (which would let `check_and_reserve` admit a further sandbox
    /// and cascade the breach). Cleared by `reconcile_counts`, which
    /// re-derives `counts` from the registry.
    #[serde(default)]
    pub ghosts: HashMap<String, usize>,
}

impl SandboxCounterData {
    /// Pop one pending reservation for `string_key`, removing the empty vec
    /// if it becomes empty. Returns `true` if a reservation was popped (so
    /// callers know whether to persist the change).
    fn pop_pending(&mut self, string_key: &str) -> bool {
        if let Some(reservations) = self.pending_reservations.get_mut(string_key) {
            reservations.pop();
            if reservations.is_empty() {
                self.pending_reservations.remove(string_key);
            }
            true
        } else {
            false
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum SandboxCounterError {
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
    max_sandbox_cnt: Option<usize>,
}

pub(crate) fn global_sandbox_config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".config").join("xiaoo").join("sandbox.toml")
}

pub(crate) fn load_max_sandbox_cnt_from_path(path: &Path) -> Option<usize> {
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

pub(crate) fn load_global_max_sandbox_cnt() -> Option<usize> {
    load_max_sandbox_cnt_from_path(&global_sandbox_config_path())
}

pub(crate) struct SandboxCounter {
    storage_path: PathBuf,
    lock_path: PathBuf,
    max_per_key: usize,
}

impl SandboxCounter {
    /// Construct with an explicit storage directory. The counter and lock
    /// files are derived from `dir`, so callers (notably tests) can isolate
    /// from the shared `~/.xiaoo/` files by pointing at a fresh temporary
    /// directory.
    pub(crate) fn new_with_storage_dir(max_per_key: usize, dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        Self {
            storage_path: dir.join("sandbox_counts.json"),
            lock_path: dir.join("sandbox_counts.lock"),
            max_per_key,
        }
    }

    pub(crate) fn max_per_key(&self) -> usize {
        self.max_per_key
    }

    pub(crate) async fn check_and_reserve(
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

    pub(crate) async fn confirm_creation(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        // Pop one pending reservation (if any). The reservation has done
        // its job of holding a slot during the build, so it is consumed
        // regardless of whether the confirmed count is incremented below.
        data.pop_pending(&string_key);

        let current = data.counts.get(&string_key).copied().unwrap_or(0);
        if current < self.max_per_key {
            data.counts.insert(string_key, current + 1);
        } else {
            // Ghost: this build's reservation was TTL-reclaimed mid-build
            // and the freed slot reused until `counts` reached the limit.
            // The real sandbox exists (the build completed), so it must be
            // accounted — but in `ghosts`, not `counts`, to preserve
            // `counts <= max_per_key`. The eventual `release` consumes a
            // ghost credit first (instead of decrementing another
            // sandbox's `counts`), which prevents the cascade breach the
            // old bare-skip caused (a ghost whose release mis-decremented
            // `counts`, dipping it below the limit and admitting a further
            // sandbox). `ghosts` is cleared by `reconcile_counts` once
            // `counts` is re-derived from the registry.
            *data.ghosts.entry(string_key.clone()).or_insert(0) += 1;
        }

        self.save_data(&data)?;
        Ok(())
    }

    pub(crate) async fn cancel_reservation(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        if data.pop_pending(&string_key) {
            self.save_data(&data)?;
        }

        Ok(())
    }

    pub async fn release(&self, key: &SandboxCounterKey) -> Result<(), SandboxCounterError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        let string_key = key.to_string_key();
        let ghosts = data.ghosts.get(&string_key).copied().unwrap_or(0);
        if ghosts > 0 {
            // This release corresponds to a ghost sandbox (one whose
            // confirm was accounted in `ghosts`, not `counts`). Consume a
            // ghost credit instead of decrementing `counts` — otherwise
            // `counts` would drop below the real live count and let
            // `check_and_reserve` admit a further sandbox (cascade
            // breach). Mis-attribution (a normal sandbox's release
            // consuming a ghost credit) is harmless: `counts + ghosts`
            // stays equal to the real live count, and `ghosts > 0`
            // implies `counts == max_per_key`, so `check_and_reserve`
            // keeps rejecting while any ghost is live.
            if ghosts > 1 {
                data.ghosts.insert(string_key, ghosts - 1);
            } else {
                data.ghosts.remove(&string_key);
            }
            self.save_data(&data)?;
            return Ok(());
        }
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
    pub(crate) async fn reconcile_counts(
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
        // `counts` is now re-derived from the registry (the source of
        // truth for live sandboxes), so the separate ghost bookkeeping is
        // no longer needed — any over-limit sandboxes are already
        // reflected in `counts` via `live_counts`. Drop all ghost credits
        // so `release` goes back to decrementing `counts` directly.
        if !data.ghosts.is_empty() {
            data.ghosts.clear();
            changed = true;
        }

        if changed {
            self.save_data(&data)?;
        }
        Ok(())
    }

    pub(crate) async fn get_total_count(
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
        super::atomic_save_json(&self.storage_path, data).map_err(|e| match e {
            super::AtomicSaveError::Io(msg) => SandboxCounterError::FileError { message: msg },
            super::AtomicSaveError::Serialize(msg) => {
                SandboxCounterError::ParseError { message: msg }
            }
        })
    }
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
#[path = "../../../../tests/unit/shared/backend/sandbox_counter_test.rs"]
mod tests;
