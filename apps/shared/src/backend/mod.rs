use agent_contracts::backend::{
    BackendCreateRequest as ProviderBackendCreateRequest, BackendDeleteRequest, BackendId,
    BackendInstance, BackendLifecycle, BackendLifecycleReason, BackendPath, BackendProvider,
    BackendResourceLimits, OperationBackend, OperationBackendBuildError, OperationError,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod backend_manager;
mod backend_registry;
mod base;
mod dirty_write;
mod e2b;
mod labels;
mod sandbox_counter;

pub use backend_manager::BackendManager;
use backend_manager::BackendManagerState;
use backend_registry::{BackendRegistry, BackendRegistryEntry, BackendRegistryError};
pub(crate) use base::{
    BackendCheckoutRequest, BackendCheckoutResult, BackendCheckpointRequest,
    BackendCheckpointResult, BackendEnsureSessionRequest, BackendLease,
};
pub use base::{
    BackendCheckpointRef, BackendInfo, BackendLineageInfo, BackendListFilter, GatewayBackendConfig,
};
// Backend checkpoint snapshot request/result types and the BackendError enum
// are internal to this crate: the app-facing surface is the `Runtime*`
// checkpoint types (via SessionControlPlane) and OperationBackendBuildError,
// never the `Backend*` counterparts. `BackendCheckpointRef` stays pub because
// it surfaces in the `SessionStore` trait method signature and the
// `SessionRecord.paused_backend_checkpoint` field. The rest are demoted to
// pub(crate).
pub use base::BackendManagerLimits;
use base::STALE_OWNER_THRESHOLD_MS;
pub(crate) use base::{
    BackendCheckpointSnapshotDeleteRequest, BackendCheckpointSnapshotDeleteResult, BackendError,
};
use dirty_write::{BackendDirtyTracker, DirtyTrackedOperationBackend};
pub use e2b::{
    build_e2b_bootstrap_archive, canonicalize_bootstrap_dir, E2bBootstrapArchive,
    E2bBootstrapBuildError,
};
pub use labels::{backend_endpoint_kind, backend_endpoint_str, backend_state_label};

// 不透明 RAII 句柄再导出：应用（endside / serverside）启动时安装进程组清理
// 守卫，无需命名底层 process_group 内部类型，故只导出该句柄。
pub use operation_backend::process_group::ProcessGroupCleanupGuard;
use sandbox_counter::{
    load_global_max_sandbox_cnt, SandboxCounter, SandboxCounterError, SandboxCounterKey,
    MAX_ACTIVE_SANDBOXES_PER_KEY,
};

struct BackendInstanceEntry {
    backend: Arc<dyn OperationBackend>,
    instance: BackendInstance,
    config: GatewayBackendConfig,
    workspace_root: String,
    config_hash: u64,
    session_ids: BTreeMap<String, ()>,
    expires_at_ms: Option<u64>,
    lineage: BackendLineageEntry,
    dirty_tracker: Arc<BackendDirtyTracker>,
}

#[derive(Debug, Clone, Default)]
struct BackendLineageEntry {
    parent_backend_id: Option<BackendId>,
    children_backend_ids: BTreeMap<String, ()>,
    forked_from_snapshot_id: Option<String>,
    forked_snapshot_names: Vec<String>,
    forked_at_ms: Option<u64>,
}

fn workspace_root_string(path: &PathBuf) -> Result<String, OperationBackendBuildError> {
    path.to_str()
        .map(str::to_string)
        .ok_or_else(|| OperationBackendBuildError::InvalidConfig {
            message: format!("workspace_root is not valid utf-8: {}", path.display()),
        })
}

fn hash_config(config: &GatewayBackendConfig) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    config.kind.hash(&mut hasher);
    canonical_json(&config.options).hash(&mut hasher);
    hasher.finish()
}

fn canonical_json(value: &Value) -> String {
    fn normalize(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), normalize(value)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(values) => Value::Array(values.iter().map(normalize).collect()),
            other => other.clone(),
        }
    }

    serde_json::to_string(&normalize(value)).unwrap_or_else(|_| "null".to_string())
}

fn resolve_session_backend_config(
    config: Option<GatewayBackendConfig>,
) -> Result<GatewayBackendConfig, OperationBackendBuildError> {
    match config {
        Some(config) if config.kind == "local" || config.kind == "e2b" => Ok(config),
        Some(config) => Err(OperationBackendBuildError::UnsupportedBackend { kind: config.kind }),
        None => Ok(GatewayBackendConfig::new(
            "local",
            default_local_provider_options(),
        )),
    }
}

fn default_local_provider_options() -> Value {
    let mut options = Map::new();
    if let Some(home_dir) = std::env::var_os("HOME") {
        options.insert(
            "home_dir".to_string(),
            Value::String(home_dir.to_string_lossy().to_string()),
        );
    }
    options.insert(
        "temp_root".to_string(),
        Value::String(std::env::temp_dir().to_string_lossy().to_string()),
    );
    Value::Object(options)
}

fn requested_backend_id(backend_id: Option<String>) -> Result<BackendId, BackendError> {
    match backend_id {
        Some(backend_id) if backend_id.trim().is_empty() => Err(BackendError::InvalidRequest {
            message: "backend_id cannot be empty".to_string(),
        }),
        Some(backend_id) => Ok(BackendId(backend_id)),
        None => Ok(new_backend_id()),
    }
}

fn new_backend_id() -> BackendId {
    BackendId(format!("bkd_{}", uuid::Uuid::new_v4().simple()))
}

fn expires_at_ms_from_timeout(timeout_secs: u64) -> u64 {
    current_time_ms().saturating_add(timeout_secs.saturating_mul(1000))
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Error from [`atomic_save_json`].
enum AtomicSaveError {
    Io(String),
    Serialize(String),
}

/// Atomically write `data` as JSON to `storage_path`: serialize into a
/// sibling temp file, fsync it, then rename it over the real path.
/// `File::create` would truncate the target immediately, so a crash
/// between truncation and a complete flush would leave the storage file
/// empty or partially written. Writing to a temp file first means the
/// live file is never observed in a torn state: a crash at any point
/// leaves either the previous contents or the fully written new contents.
fn atomic_save_json<T: serde::Serialize>(
    storage_path: &Path,
    data: &T,
) -> Result<(), AtomicSaveError> {
    let parent = storage_path
        .parent()
        .ok_or_else(|| AtomicSaveError::Io("storage_path has no parent directory".to_string()))?;
    let file_name = storage_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("data.json");
    let temp_path = parent.join(format!(".{}.tmp", file_name));

    let mut file = File::create(&temp_path)
        .map_err(|e| AtomicSaveError::Io(format!("failed to create temp storage file: {}", e)))?;

    let mut writer = BufWriter::new(&mut file);
    serde_json::to_writer(&mut writer, data)
        .map_err(|e| AtomicSaveError::Serialize(format!("failed to write storage file: {}", e)))?;
    writer
        .flush()
        .map_err(|e| AtomicSaveError::Io(format!("failed to flush storage file: {}", e)))?;
    // Drop the BufWriter borrow so we can fsync the underlying File.
    drop(writer);
    file.sync_all()
        .map_err(|e| AtomicSaveError::Io(format!("failed to sync storage file: {}", e)))?;
    drop(file);

    if let Err(e) = std::fs::rename(&temp_path, storage_path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(AtomicSaveError::Io(format!(
            "failed to rename storage file: {}",
            e
        )));
    }

    Ok(())
}

fn metadata_matches_filter(metadata: &Value, filter: &BTreeMap<String, String>) -> bool {
    if filter.is_empty() {
        return true;
    }
    let Some(object) = metadata.as_object() else {
        return false;
    };
    filter.iter().all(|(key, expected)| {
        object
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|actual| actual == expected)
    })
}

fn forked_provider_options(
    parent_options: &Value,
    override_options: Option<&Value>,
    snapshot_id: &str,
) -> Value {
    let mut options = parent_options.as_object().cloned().unwrap_or_default();
    if let Some(overrides) = override_options.and_then(Value::as_object) {
        for (key, value) in overrides {
            options.insert(key.clone(), value.clone());
        }
    }
    options.insert(
        "template_id".to_string(),
        Value::String(snapshot_id.to_string()),
    );
    Value::Object(options)
}

fn detach_from_parent(
    state: &mut BackendManagerState,
    backend_id: &BackendId,
    entry: &BackendInstanceEntry,
) {
    let Some(parent_backend_id) = entry.lineage.parent_backend_id.as_ref() else {
        return;
    };
    if let Some(parent) = state.backends.get_mut(parent_backend_id) {
        parent.lineage.children_backend_ids.remove(&backend_id.0);
    } else {
        // Parent backend was already removed (evicted or released). The
        // child's `parent_backend_id` is now a dangling reference, but since
        // the child entry itself is being removed by the caller right after
        // this call, the dangling ref does not persist. Log for observability.
        tracing::debug!(
            backend_id = %backend_id.0,
            parent_backend_id = %parent_backend_id.0,
            "parent backend already removed during detach; child entry is being deleted"
        );
    }
}

struct BuildBackendInput {
    backend_id: BackendId,
    config: GatewayBackendConfig,
    workspace_root_text: String,
    config_hash: u64,
    session_id_for_instance: String,
    session_id: Option<String>,
    resource_limits: BackendResourceLimits,
    metadata: Value,
    expires_at_ms: Option<u64>,
    lineage: BackendLineageEntry,
    backend_checkpoint: Option<BackendCheckpointRef>,
    e2b_bootstrap: Option<Arc<E2bBootstrapArchive>>,
}

async fn build_backend(input: BuildBackendInput) -> Result<BackendInstanceEntry, BackendError> {
    if input.config.kind == "e2b" {
        let created = e2b::create_backend(e2b::E2bCreateBackendInput {
            backend_id: input.backend_id,
            session_id_for_instance: input.session_id_for_instance,
            workspace_root_text: input.workspace_root_text.clone(),
            provider_options: input.config.options.clone(),
            resource_limits: input.resource_limits,
            metadata: input.metadata,
            bootstrap: input.e2b_bootstrap,
        })
        .await?;
        let dirty_tracker = Arc::new(BackendDirtyTracker::default());
        if let Some(checkpoint) = input.backend_checkpoint {
            dirty_tracker.set_checkpoint(checkpoint);
        }
        return Ok(BackendInstanceEntry {
            backend: DirtyTrackedOperationBackend::wrap(
                created.backend,
                Arc::clone(&dirty_tracker),
            ),
            instance: created.instance,
            config: input.config,
            workspace_root: input.workspace_root_text,
            config_hash: input.config_hash,
            session_ids: input
                .session_id
                .map(|session_id| BTreeMap::from([(session_id, ())]))
                .unwrap_or_default(),
            expires_at_ms: input.expires_at_ms,
            lineage: input.lineage,
            dirty_tracker,
        });
    }

    let provider = local_provider_for_kind(&input.config.kind)?;
    let lifecycle = provider.lifecycle();
    let instance = lifecycle
        .create_sandbox(ProviderBackendCreateRequest {
            requested_backend_id: Some(input.backend_id),
            session_id: input.session_id_for_instance,
            conversation_id: None,
            workspace_root: BackendPath::from_raw(input.workspace_root_text.clone()),
            provider_options: input.config.options.clone(),
            resource_limits: input.resource_limits,
            metadata: input.metadata,
        })
        .await
        .map_err(BackendError::from_control_error)?;
    let backend = provider
        .attach(instance.clone())
        .await
        .map_err(BackendError::from_control_error)?;
    let dirty_tracker = Arc::new(BackendDirtyTracker::default());
    if let Some(checkpoint) = input.backend_checkpoint {
        dirty_tracker.set_checkpoint(checkpoint);
    }
    Ok(BackendInstanceEntry {
        backend: DirtyTrackedOperationBackend::wrap(backend, Arc::clone(&dirty_tracker)),
        instance,
        config: input.config,
        workspace_root: input.workspace_root_text,
        config_hash: input.config_hash,
        session_ids: input
            .session_id
            .map(|session_id| BTreeMap::from([(session_id, ())]))
            .unwrap_or_default(),
        expires_at_ms: input.expires_at_ms,
        lineage: input.lineage,
        dirty_tracker,
    })
}

fn local_provider_for_kind(
    kind: &str,
) -> Result<operation_backend::LocalBackendProvider, BackendError> {
    match kind {
        "local" => Ok(operation_backend::local_backend_provider()),
        other => Err(BackendError::UnsupportedBackend {
            kind: other.to_string(),
        }),
    }
}

async fn delete_backend_instance(
    instance: BackendInstanceEntry,
    reason: BackendLifecycleReason,
) -> Result<(), OperationError> {
    if instance.config.kind == "e2b" {
        instance.backend.shutdown().await?;
        return Ok(());
    }

    let provider = local_provider_for_kind(&instance.config.kind)
        .map_err(BackendError::into_operation_error)?;
    provider
        .delete(BackendDeleteRequest {
            backend_id: instance.instance.backend_id,
            instance_id: Some(instance.instance.instance_id),
            snapshot_id: instance
                .instance
                .snapshot
                .map(|snapshot| snapshot.snapshot_id),
            force: false,
            reason,
            metadata: instance.instance.metadata,
        })
        .await
        .map_err(|error| OperationError::Transport {
            message: error.to_string(),
        })?;
    Ok(())
}

impl BackendInstanceEntry {
    fn info(&self) -> BackendInfo {
        let session_ids = self.session_ids.keys().cloned().collect::<Vec<_>>();
        BackendInfo {
            backend_id: self.instance.backend_id.0.clone(),
            provider: self.instance.provider.0.clone(),
            instance_id: self.instance.instance_id.0.clone(),
            state: self.instance.state,
            workspace_root: self.instance.workspace_root.native().to_string(),
            endpoint: self.instance.endpoint.clone(),
            metadata: self.instance.metadata.clone(),
            resources: self.instance.resources,
            session_id: if session_ids.len() == 1 {
                session_ids.first().cloned()
            } else {
                None
            },
            session_ids,
            expires_at_ms: self.expires_at_ms,
            lineage: BackendLineageInfo {
                parent_backend_id: self
                    .lineage
                    .parent_backend_id
                    .as_ref()
                    .map(|id| id.0.clone()),
                children_backend_ids: self.lineage.children_backend_ids.keys().cloned().collect(),
                forked_from_snapshot_id: self.lineage.forked_from_snapshot_id.clone(),
                forked_snapshot_names: self.lineage.forked_snapshot_names.clone(),
                forked_at_ms: self.lineage.forked_at_ms,
            },
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/unit/shared/backend_test.rs"]
mod tests;
