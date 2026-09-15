use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::os::unix::io::AsRawFd;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

use super::sandbox_counter::SandboxCounterKey;

static IN_PROCESS_LOCK: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SessionStatusSnapshot {
    pub status: String,
    pub queue_depth: usize,
    pub updated_at_ms: u64,
}

impl Default for SessionStatusSnapshot {
    fn default() -> Self {
        Self {
            status: "idle".to_string(),
            queue_depth: 0,
            updated_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct BackendRegistryEntry {
    pub backend_id: String,
    pub sandbox_key: SandboxCounterKey,
    pub session_ids: Vec<String>,
    pub owner_process_id: String,
    pub session_statuses: HashMap<String, SessionStatusSnapshot>,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub pending_eviction: bool,
    /// Provider-side sandbox id (e.g. the e2b sandbox id). Used by any
    /// process to delete the remote sandbox when reclaiming an orphaned
    /// backend whose owner process has died. The reclaiming process uses its
    /// own provider_options (same api_key, since the sandbox_key matches) to
    /// authorise the delete call, so no secrets are persisted here.
    #[serde(default)]
    pub instance_id: String,
    /// Last time the owner process touched this entry. If
    /// `now - owner_heartbeat_ms > STALE_OWNER_THRESHOLD_MS` the owner is
    /// presumed dead and the backend may be reclaimed by any process.
    #[serde(default)]
    pub owner_heartbeat_ms: u64,
    /// OS process ID of the owner. Used by other processes to send SIGUSR1
    /// (cross-process eviction signal) to request immediate eviction of an
    /// idle sandbox without waiting for the periodic heartbeat tick.
    #[serde(default)]
    pub owner_pid: u32,
}

impl BackendRegistryEntry {
    pub(crate) fn new(
        backend_id: String,
        sandbox_key: SandboxCounterKey,
        session_ids: Vec<String>,
        owner_process_id: String,
        instance_id: String,
        initial_session_status: Option<(&str, usize)>,
    ) -> Self {
        let now_ms = current_time_ms();
        let session_statuses = session_ids
            .iter()
            .map(|id| {
                let snapshot = match initial_session_status {
                    Some((status, queue_depth)) => SessionStatusSnapshot {
                        status: status.to_string(),
                        queue_depth,
                        updated_at_ms: now_ms,
                    },
                    None => SessionStatusSnapshot::default(),
                };
                (id.clone(), snapshot)
            })
            .collect();
        Self {
            backend_id,
            sandbox_key,
            session_ids,
            owner_process_id,
            session_statuses,
            created_at_ms: now_ms,
            last_activity_ms: now_ms,
            pending_eviction: false,
            instance_id,
            owner_heartbeat_ms: now_ms,
            owner_pid: std::process::id(),
        }
    }

    pub(crate) fn update_session_status(
        &mut self,
        session_id: &str,
        status: &str,
        queue_depth: usize,
    ) {
        let snapshot = SessionStatusSnapshot {
            status: status.to_string(),
            queue_depth,
            updated_at_ms: current_time_ms(),
        };
        self.session_statuses
            .insert(session_id.to_string(), snapshot);
    }

    pub(crate) fn is_all_sessions_idle(&self) -> bool {
        self.session_statuses
            .values()
            .all(|s| s.status == "idle" && s.queue_depth == 0)
    }

    /// True if the owner process is presumed dead (no heartbeat within the
    /// threshold).
    pub(crate) fn is_owner_stale(&self, threshold_ms: u64) -> bool {
        if self.owner_heartbeat_ms == 0 {
            return true;
        }
        current_time_ms().saturating_sub(self.owner_heartbeat_ms) > threshold_ms
    }

    /// A backend is evictable if it is not already pending eviction and
    /// either all its sessions are idle (the normal path) or its owner
    /// process is presumed dead (the orphan-reclaim path).
    ///
    /// Use this in selection paths (e.g. `try_evict_if_needed`) to skip
    /// backends that another process has already marked for eviction.
    pub(crate) fn is_evictable(&self, stale_threshold_ms: u64) -> bool {
        !self.pending_eviction && self.is_eviction_safe(stale_threshold_ms)
    }

    /// True if the backend's current state permits eviction to proceed:
    /// either all its sessions are idle (the normal path) or its owner
    /// process is presumed dead (the orphan-reclaim path).
    ///
    /// Unlike `is_evictable`, this does NOT consider `pending_eviction` —
    /// use it in the actual eviction path (e.g.
    /// `check_and_evict_marked_backends`) where the entry is already known
    /// to be marked and we only need to confirm it is still safe to delete.
    pub(crate) fn is_eviction_safe(&self, stale_threshold_ms: u64) -> bool {
        self.is_all_sessions_idle() || self.is_owner_stale(stale_threshold_ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct BackendRegistryData {
    pub entries: HashMap<String, BackendRegistryEntry>,
}

#[derive(Debug, Clone)]
pub(crate) enum BackendRegistryError {
    FileError { message: String },
    LockError { message: String },
    ParseError { message: String },
}

impl std::fmt::Display for BackendRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::FileError { message } => write!(f, "file error: {}", message),
            Self::LockError { message } => write!(f, "lock error: {}", message),
            Self::ParseError { message } => write!(f, "parse error: {}", message),
        }
    }
}

impl std::error::Error for BackendRegistryError {}

pub(crate) struct BackendRegistry {
    storage_path: PathBuf,
    lock_path: PathBuf,
}

impl BackendRegistry {
    /// Construct with an explicit storage directory. The registry and lock
    /// files are derived from `dir`, so callers (notably tests) can isolate
    /// from the shared `~/.xiaoo/` files by pointing at a fresh temporary
    /// directory.
    pub(crate) fn new_with_storage_dir(dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).ok();
        Self {
            storage_path: dir.join("backend_registry.json"),
            lock_path: dir.join("backend_registry.lock"),
        }
    }

    pub async fn register(&self, entry: BackendRegistryEntry) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        data.entries.insert(entry.backend_id.clone(), entry);
        self.save_data(&data)?;

        Ok(())
    }

    pub(crate) async fn unregister(&self, backend_id: &str) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        data.entries.remove(backend_id);
        self.save_data(&data)?;

        Ok(())
    }

    pub(crate) async fn get_entries_by_key(
        &self,
        key: &SandboxCounterKey,
    ) -> Result<Vec<BackendRegistryEntry>, BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;

        Ok(data
            .entries
            .values()
            .filter(|entry| entry.sandbox_key == *key)
            .cloned()
            .collect())
    }

    pub(crate) async fn get_all_entries(
        &self,
    ) -> Result<Vec<BackendRegistryEntry>, BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;

        Ok(data.entries.values().cloned().collect())
    }

    pub(crate) async fn set_pending_eviction(
        &self,
        backend_id: &str,
        pending: bool,
    ) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        if let Some(entry) = data.entries.get_mut(backend_id) {
            entry.pending_eviction = pending;
            self.save_data(&data)?;
        }

        Ok(())
    }

    pub(crate) async fn update_activity(
        &self,
        backend_id: &str,
    ) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        if let Some(entry) = data.entries.get_mut(backend_id) {
            entry.last_activity_ms = current_time_ms();
            self.save_data(&data)?;
        }

        Ok(())
    }

    /// Refresh the heartbeat timestamp for every backend owned by
    /// `process_id`. Called periodically by the owner process's registry
    /// poller so that other processes can detect when this process has died
    /// (heartbeat goes stale) and reclaim its sandboxes.
    pub(crate) async fn refresh_heartbeats_for_process(
        &self,
        process_id: &str,
    ) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;
        let now = current_time_ms();
        let mut changed = false;
        for entry in data.entries.values_mut() {
            if entry.owner_process_id == process_id {
                entry.owner_heartbeat_ms = now;
                changed = true;
            }
        }
        if changed {
            self.save_data(&data)?;
        }
        Ok(())
    }

    pub(crate) async fn update_session_status(
        &self,
        backend_id: &str,
        session_id: &str,
        status: &str,
        queue_depth: usize,
    ) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        if let Some(entry) = data.entries.get_mut(backend_id) {
            entry.update_session_status(session_id, status, queue_depth);
            entry.last_activity_ms = current_time_ms();
            self.save_data(&data)?;
        }

        Ok(())
    }

    pub(crate) async fn get_entries_for_process(
        &self,
        process_id: &str,
    ) -> Result<Vec<BackendRegistryEntry>, BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;

        Ok(data
            .entries
            .values()
            .filter(|entry| entry.owner_process_id == process_id)
            .cloned()
            .collect())
    }

    fn acquire_lock(&self) -> Result<File, BackendRegistryError> {
        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|e| BackendRegistryError::LockError {
                message: format!("failed to create lock file: {}", e),
            })?;

        let fd = lock_file.as_raw_fd();
        let result = unsafe { libc::flock(fd, libc::LOCK_EX) };
        if result != 0 {
            return Err(BackendRegistryError::LockError {
                message: format!(
                    "failed to acquire lock: {}",
                    std::io::Error::last_os_error()
                ),
            });
        }

        Ok(lock_file)
    }

    fn load_data(&self) -> Result<BackendRegistryData, BackendRegistryError> {
        if !self.storage_path.exists() {
            return Ok(BackendRegistryData::default());
        }

        let file = File::open(&self.storage_path).map_err(|e| BackendRegistryError::FileError {
            message: format!("failed to open storage file: {}", e),
        })?;

        let reader = BufReader::new(file);
        serde_json::from_reader(reader).map_err(|e| BackendRegistryError::ParseError {
            message: format!("failed to parse storage file: {}", e),
        })
    }

    fn save_data(&self, data: &BackendRegistryData) -> Result<(), BackendRegistryError> {
        super::atomic_save_json(&self.storage_path, data).map_err(|e| match e {
            super::AtomicSaveError::Io(msg) => BackendRegistryError::FileError { message: msg },
            super::AtomicSaveError::Serialize(msg) => {
                BackendRegistryError::ParseError { message: msg }
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
#[path = "../../../../tests/unit/shared/backend/backend_registry_test.rs"]
mod tests;
