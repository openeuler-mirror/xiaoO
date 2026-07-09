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
pub struct SessionStatusSnapshot {
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
pub struct BackendRegistryEntry {
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
    pub fn new(
        backend_id: String,
        sandbox_key: SandboxCounterKey,
        session_ids: Vec<String>,
        owner_process_id: String,
        instance_id: String,
    ) -> Self {
        let now_ms = current_time_ms();
        let session_statuses = session_ids
            .iter()
            .map(|id| (id.clone(), SessionStatusSnapshot::default()))
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

    #[cfg(test)]
    pub(crate) fn get_session_status(&self, session_id: &str) -> Option<&SessionStatusSnapshot> {
        self.session_statuses.get(session_id)
    }

    pub(crate) fn is_all_sessions_idle(&self) -> bool {
        self.session_statuses
            .values()
            .all(|s| s.status == "idle" && s.queue_depth == 0)
    }

    /// True if the owner process is presumed dead (no heartbeat within the
    /// threshold).
    pub fn is_owner_stale(&self, threshold_ms: u64) -> bool {
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
    pub fn is_evictable(&self, stale_threshold_ms: u64) -> bool {
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
    pub fn is_eviction_safe(&self, stale_threshold_ms: u64) -> bool {
        self.is_all_sessions_idle() || self.is_owner_stale(stale_threshold_ms)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BackendRegistryData {
    pub entries: HashMap<String, BackendRegistryEntry>,
}

#[derive(Debug, Clone)]
pub enum BackendRegistryError {
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

pub struct BackendRegistry {
    storage_path: PathBuf,
    lock_path: PathBuf,
}

impl BackendRegistry {
    /// Construct with an explicit storage directory. The registry and lock
    /// files are derived from `dir`, so callers (notably tests) can isolate
    /// from the shared `~/.xiaoo/` files by pointing at a fresh temporary
    /// directory.
    pub fn new_with_storage_dir(dir: PathBuf) -> Self {
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

    pub async fn unregister(&self, backend_id: &str) -> Result<(), BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let mut data = self.load_data()?;

        data.entries.remove(backend_id);
        self.save_data(&data)?;

        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn get_entry(
        &self,
        backend_id: &str,
    ) -> Result<Option<BackendRegistryEntry>, BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;

        Ok(data.entries.get(backend_id).cloned())
    }

    pub async fn get_entries_by_key(
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

    pub async fn get_all_entries(&self) -> Result<Vec<BackendRegistryEntry>, BackendRegistryError> {
        let _in_process = IN_PROCESS_LOCK.lock().await;
        let _file_lock = self.acquire_lock()?;
        let data = self.load_data()?;

        Ok(data.entries.values().cloned().collect())
    }

    pub async fn set_pending_eviction(
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

    pub async fn update_activity(&self, backend_id: &str) -> Result<(), BackendRegistryError> {
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
    pub async fn refresh_heartbeats_for_process(
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

    pub async fn update_session_status(
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

    pub async fn get_entries_for_process(
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
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Build a registry backed by a fresh temporary directory so the test
    /// never touches the shared `~/.xiaoo/backend_registry.json` used by a
    /// running daemon. The returned `TempDir` must outlive the registry.
    fn temp_registry() -> (TempDir, BackendRegistry) {
        let dir = TempDir::new().expect("tempdir");
        let registry = BackendRegistry::new_with_storage_dir(dir.path().to_path_buf());
        (dir, registry)
    }

    #[tokio::test]
    async fn test_register_and_unregister() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-register");
        let entry = BackendRegistryEntry::new(
            "backend-1".to_string(),
            key,
            vec!["session-1".to_string()],
            "process-1".to_string(),
            "e2b-sandbox-1".to_string(),
        );

        registry.register(entry).await.expect("should register");

        let retrieved = registry
            .get_entry("backend-1")
            .await
            .expect("should get entry");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().backend_id, "backend-1");

        registry
            .unregister("backend-1")
            .await
            .expect("should unregister");

        let retrieved = registry
            .get_entry("backend-1")
            .await
            .expect("should get entry");
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_get_entries_by_key() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-key-multi-v2");

        for i in 0..3 {
            let entry = BackendRegistryEntry::new(
                format!("backend-mv2-{}", i),
                key.clone(),
                vec![format!("session-mv2-{}", i)],
                format!("process-mv2-{}", i),
                format!("e2b-sandbox-mv2-{}", i),
            );
            registry.register(entry).await.expect("should register");
        }

        let entries = registry
            .get_entries_by_key(&key)
            .await
            .expect("should get entries");
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn test_pending_eviction() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-eviction");
        let entry = BackendRegistryEntry::new(
            "backend-eviction-v2".to_string(),
            key,
            vec!["session-eviction".to_string()],
            "process-eviction".to_string(),
            "e2b-sandbox-eviction".to_string(),
        );

        registry.register(entry).await.expect("should register");

        registry
            .set_pending_eviction("backend-eviction-v2", true)
            .await
            .expect("should set pending");

        let retrieved = registry
            .get_entry("backend-eviction-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");
        assert!(retrieved.pending_eviction);

        registry
            .set_pending_eviction("backend-eviction-v2", false)
            .await
            .expect("should clear pending");

        let retrieved = registry
            .get_entry("backend-eviction-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");
        assert!(!retrieved.pending_eviction);
    }

    #[tokio::test]
    async fn test_update_activity() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-activity");
        let entry = BackendRegistryEntry::new(
            "backend-activity-v2".to_string(),
            key,
            vec!["session-activity".to_string()],
            "process-activity".to_string(),
            "e2b-sandbox-activity".to_string(),
        );

        registry
            .register(entry.clone())
            .await
            .expect("should register");

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        registry
            .update_activity("backend-activity-v2")
            .await
            .expect("should update activity");

        let retrieved = registry
            .get_entry("backend-activity-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");
        assert!(retrieved.last_activity_ms > entry.last_activity_ms);
    }

    #[tokio::test]
    async fn test_get_entries_for_process() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-process-v2");

        let entry1 = BackendRegistryEntry::new(
            "backend-pv2-1".to_string(),
            key.clone(),
            vec!["session-p1".to_string()],
            "process-A".to_string(),
            "e2b-sandbox-p1".to_string(),
        );
        let entry2 = BackendRegistryEntry::new(
            "backend-pv2-2".to_string(),
            key,
            vec!["session-p2".to_string()],
            "process-A".to_string(),
            "e2b-sandbox-p2".to_string(),
        );
        let entry3 = BackendRegistryEntry::new(
            "backend-pv2-3".to_string(),
            SandboxCounterKey::new("e2b", "other-key"),
            vec!["session-p3".to_string()],
            "process-B".to_string(),
            "e2b-sandbox-p3".to_string(),
        );

        registry.register(entry1).await.expect("should register");
        registry.register(entry2).await.expect("should register");
        registry.register(entry3).await.expect("should register");

        let entries_a = registry
            .get_entries_for_process("process-A")
            .await
            .expect("should get entries");
        assert_eq!(entries_a.len(), 2);

        let entries_b = registry
            .get_entries_for_process("process-B")
            .await
            .expect("should get entries");
        assert_eq!(entries_b.len(), 1);
    }

    #[tokio::test]
    async fn test_update_session_status() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-status-v2");
        let entry = BackendRegistryEntry::new(
            "backend-status-v2".to_string(),
            key,
            vec!["session-1".to_string()],
            "process-1".to_string(),
            "e2b-sandbox-status".to_string(),
        );

        registry.register(entry).await.expect("should register");

        registry
            .update_session_status("backend-status-v2", "session-1", "running", 1)
            .await
            .expect("should update status");

        let retrieved = registry
            .get_entry("backend-status-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");

        let status = retrieved
            .get_session_status("session-1")
            .expect("should have status");
        assert_eq!(status.status, "running");
        assert_eq!(status.queue_depth, 1);

        registry
            .update_session_status("backend-status-v2", "session-1", "idle", 0)
            .await
            .expect("should update status");

        let retrieved = registry
            .get_entry("backend-status-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");

        assert!(retrieved.is_all_sessions_idle());
    }

    #[tokio::test]
    async fn test_is_all_sessions_idle() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-idle-v2");
        let entry = BackendRegistryEntry::new(
            "backend-idle-v2".to_string(),
            key,
            vec!["session-1".to_string(), "session-2".to_string()],
            "process-1".to_string(),
            "e2b-sandbox-idle".to_string(),
        );

        registry.register(entry).await.expect("should register");

        registry
            .update_session_status("backend-idle-v2", "session-1", "idle", 0)
            .await
            .expect("should update status");
        registry
            .update_session_status("backend-idle-v2", "session-2", "idle", 0)
            .await
            .expect("should update status");

        let retrieved = registry
            .get_entry("backend-idle-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");
        assert!(retrieved.is_all_sessions_idle());

        registry
            .update_session_status("backend-idle-v2", "session-1", "running", 1)
            .await
            .expect("should update status");

        let retrieved = registry
            .get_entry("backend-idle-v2")
            .await
            .expect("should get entry")
            .expect("should have entry");
        assert!(!retrieved.is_all_sessions_idle());
    }

    #[tokio::test]
    async fn test_refresh_heartbeats_for_process() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-heartbeat");

        let entry_a = BackendRegistryEntry::new(
            "backend-hb-a".to_string(),
            key.clone(),
            vec!["session-a".to_string()],
            "process-A".to_string(),
            "e2b-hb-a".to_string(),
        );
        let entry_b = BackendRegistryEntry::new(
            "backend-hb-b".to_string(),
            key,
            vec!["session-b".to_string()],
            "process-B".to_string(),
            "e2b-hb-b".to_string(),
        );
        registry.register(entry_a).await.expect("register a");
        registry.register(entry_b).await.expect("register b");

        // Capture B's heartbeat, then refresh A's.
        let before_b = registry
            .get_entry("backend-hb-b")
            .await
            .expect("get b")
            .expect("has b")
            .owner_heartbeat_ms;
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        registry
            .refresh_heartbeats_for_process("process-A")
            .await
            .expect("refresh");

        let a = registry
            .get_entry("backend-hb-a")
            .await
            .expect("get a")
            .expect("has a");
        let b = registry
            .get_entry("backend-hb-b")
            .await
            .expect("get b")
            .expect("has b");
        // A was refreshed, B was not touched.
        assert!(a.owner_heartbeat_ms >= b.owner_heartbeat_ms);
        assert_eq!(b.owner_heartbeat_ms, before_b);
    }

    #[tokio::test]
    async fn test_is_evictable_when_owner_stale() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-stale-owner");
        let mut entry = BackendRegistryEntry::new(
            "backend-stale".to_string(),
            key,
            vec!["session-1".to_string()],
            "process-dead".to_string(),
            "e2b-sandbox-stale".to_string(),
        );
        // Session is running (not idle), but owner heartbeat is very old.
        entry.update_session_status("session-1", "running", 1);
        entry.owner_heartbeat_ms = current_time_ms().saturating_sub(60_000);
        registry.register(entry).await.expect("register");

        let retrieved = registry
            .get_entry("backend-stale")
            .await
            .expect("get")
            .expect("has entry");
        // Running session → not idle, but stale owner → evictable.
        assert!(!retrieved.is_all_sessions_idle());
        assert!(retrieved.is_owner_stale(30_000));
        assert!(retrieved.is_evictable(30_000));
    }

    #[tokio::test]
    async fn test_is_evictable_when_idle_and_owner_alive() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-alive-owner");
        let mut entry = BackendRegistryEntry::new(
            "backend-alive".to_string(),
            key,
            vec!["session-1".to_string()],
            "process-alive".to_string(),
            "e2b-sandbox-alive".to_string(),
        );
        entry.update_session_status("session-1", "idle", 0);
        registry.register(entry).await.expect("register");

        let retrieved = registry
            .get_entry("backend-alive")
            .await
            .expect("get")
            .expect("has entry");
        assert!(retrieved.is_all_sessions_idle());
        assert!(!retrieved.is_owner_stale(30_000));
        assert!(retrieved.is_evictable(30_000));
    }

    #[tokio::test]
    async fn test_not_evictable_when_running_and_owner_alive() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-busy-owner");
        let mut entry = BackendRegistryEntry::new(
            "backend-busy".to_string(),
            key,
            vec!["session-1".to_string()],
            "process-alive".to_string(),
            "e2b-sandbox-busy".to_string(),
        );
        entry.update_session_status("session-1", "running", 1);
        registry.register(entry).await.expect("register");

        let retrieved = registry
            .get_entry("backend-busy")
            .await
            .expect("get")
            .expect("has entry");
        assert!(!retrieved.is_all_sessions_idle());
        assert!(!retrieved.is_owner_stale(30_000));
        assert!(!retrieved.is_evictable(30_000));
    }

    /// Regression test for the cross-process eviction bug.
    ///
    /// When a backend is marked `pending_eviction=true` (by another process
    /// requesting eviction via SIGUSR1), `is_evictable` returns false (it
    /// excludes already-pending entries so they aren't selected twice).
    /// The owner process's `check_and_evict_marked_backends` handler must
    /// still be able to confirm the backend is safe to delete via
    /// `is_eviction_safe` — otherwise the handler would clear the mark and
    /// skip every time, never actually evicting, and the requesting process
    /// would fail with "sandbox limit reached" even though idle sandboxes
    /// are available to evict.
    #[tokio::test]
    async fn test_is_eviction_safe_ignores_pending_eviction_for_marked_entry() {
        let (_dir, registry) = temp_registry();
        let key = SandboxCounterKey::new("e2b", "test-marked-eviction");
        let mut entry = BackendRegistryEntry::new(
            "backend-marked".to_string(),
            key,
            vec!["session-1".to_string()],
            "process-alive".to_string(),
            "e2b-sandbox-marked".to_string(),
        );
        // Session is idle and owner is alive — the normal "evict me" case.
        entry.update_session_status("session-1", "idle", 0);
        // Another process has marked this backend for eviction.
        entry.pending_eviction = true;
        registry.register(entry).await.expect("register");

        let retrieved = registry
            .get_entry("backend-marked")
            .await
            .expect("get")
            .expect("has entry");

        // `is_evictable` returns false because pending_eviction is true —
        // this is what `try_evict_if_needed` uses to skip already-marked
        // entries when selecting a victim.
        assert!(!retrieved.is_evictable(30_000));
        // But `is_eviction_safe` must return true so the owner's signal
        // handler actually performs the eviction instead of clearing the
        // mark and skipping.
        assert!(retrieved.is_eviction_safe(30_000));
    }
}
