use agent_contracts::backend::{
    BackendControlError, BackendCreateRequest as ProviderBackendCreateRequest,
    BackendDeleteRequest, BackendId, BackendInstance, BackendLifecycle, BackendLifecycleReason,
    BackendPath, BackendProvider, BackendResourceLimits, OperationBackend,
    OperationBackendBuildError, OperationError,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

mod backend_manager;
mod base;
mod dirty_write;
mod e2b;

pub use backend_manager::BackendManager;
use backend_manager::BackendManagerState;
pub use base::{
    BackendCheckoutRequest, BackendCheckoutResult, BackendCheckpointRef, BackendCheckpointRequest,
    BackendCheckpointResult, BackendConnectRequest, BackendCreateRequest,
    BackendEnsureSessionRequest, BackendError, BackendForkRequest, BackendForkResult, BackendInfo,
    BackendLease, BackendLineageInfo, BackendListFilter, BackendTreeNode, GatewayBackendConfig,
};
use dirty_write::{BackendDirtyTracker, DirtyTrackedOperationBackend};

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

fn resolve_backend_config(
    provider: Option<String>,
    options: Option<Value>,
) -> Result<GatewayBackendConfig, BackendError> {
    let kind = provider.unwrap_or_else(|| "local".to_string());
    let options = options.unwrap_or_else(|| {
        if kind == "local" {
            default_local_provider_options()
        } else {
            Value::Object(Map::new())
        }
    });
    resolve_session_backend_config(Some(GatewayBackendConfig::new(kind, options)))
        .map_err(BackendError::from_build_error)
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
    }
}

fn backend_tree_node(
    state: &BackendManagerState,
    backend_id: &BackendId,
) -> Option<BackendTreeNode> {
    let entry = state.backends.get(backend_id)?;
    let mut child_ids = entry
        .lineage
        .children_backend_ids
        .keys()
        .filter_map(|id| {
            state
                .backends
                .contains_key(&BackendId(id.clone()))
                .then(|| id.clone())
        })
        .collect::<Vec<_>>();
    child_ids.sort();
    let children = child_ids
        .into_iter()
        .filter_map(|id| backend_tree_node(state, &BackendId(id)))
        .collect();
    Some(BackendTreeNode {
        backend: entry.info(),
        children,
    })
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
            workspace_root: BackendPath(input.workspace_root_text.clone()),
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
        .map_err(control_error_to_operation_error)?;
    Ok(())
}

fn control_error_to_operation_error(error: BackendControlError) -> OperationError {
    OperationError::Transport {
        message: error.to_string(),
    }
}

impl BackendInstanceEntry {
    fn info(&self) -> BackendInfo {
        let session_ids = self.session_ids.keys().cloned().collect::<Vec<_>>();
        BackendInfo {
            backend_id: self.instance.backend_id.0.clone(),
            provider: self.instance.provider.0.clone(),
            instance_id: self.instance.instance_id.0.clone(),
            state: self.instance.state,
            workspace_root: self.instance.workspace_root.0.clone(),
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
mod tests {
    use super::*;
    use agent_contracts::backend::{
        capability::{
            exec::ExecRequest,
            filesystem::{WriteBytesRequest, WriteMode},
        },
        BackendLifecycle, BackendLifecycleState, BackendPath, BackendPauseMode,
        BackendPauseRequest,
    };
    use serde_json::json;
    use tempfile::TempDir;

    fn local_request(
        session_id: &str,
        workspace_root: PathBuf,
        options: Value,
    ) -> BackendEnsureSessionRequest {
        BackendEnsureSessionRequest {
            config: Some(GatewayBackendConfig::new("local", options)),
            workspace_root,
            session_id: session_id.to_string(),
        }
    }

    fn temp_options(workspace: &TempDir) -> Value {
        json!({"temp_root": workspace.path().to_string_lossy().to_string()})
    }

    #[test]
    fn backend_config_hash_is_stable_for_reordered_config() {
        let left = GatewayBackendConfig::new(
            "local",
            json!({"home_dir": "/home/user", "temp_root": "/tmp/xiaoo"}),
        );
        let right = GatewayBackendConfig::new(
            "local",
            json!({"temp_root": "/tmp/xiaoo", "home_dir": "/home/user"}),
        );

        assert_eq!(hash_config(&left), hash_config(&right));
    }

    #[test]
    fn non_local_backend_is_unsupported() {
        let config =
            resolve_session_backend_config(Some(GatewayBackendConfig::new("docker", Value::Null)));

        assert!(matches!(
            config,
            Err(OperationBackendBuildError::UnsupportedBackend { kind }) if kind == "docker"
        ));
    }

    #[tokio::test]
    async fn manager_reuses_backend_for_same_session_and_config() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let request = local_request(
            "s1",
            workspace.path().to_path_buf(),
            temp_options(&workspace),
        );

        let first = manager
            .ensure_session_backend(request.clone())
            .await
            .expect("first lease");
        let second = manager
            .ensure_session_backend(request)
            .await
            .expect("second lease");

        let first_backend = first.backend();
        let second_backend = second.backend();
        assert_eq!(first.instance(), second.instance());
        assert!(first.instance().backend_id.0.starts_with("bkd_"));
        assert_ne!(first.instance().backend_id.0, "s1");
        assert!(Arc::ptr_eq(&first_backend, &second_backend));
    }

    #[tokio::test]
    async fn manager_does_not_implicitly_reuse_backend_across_sessions() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let root = workspace.path().to_path_buf();
        let base = local_request("s1", root.clone(), temp_options(&workspace));
        let other_session = local_request("s2", root.clone(), temp_options(&workspace));
        let other_config = local_request(
            "s1",
            root,
            json!({
                "temp_root": workspace.path().to_string_lossy().to_string(),
                "default_shell": "/bin/sh"
            }),
        );

        let base_backend = manager
            .ensure_session_backend(base)
            .await
            .expect("base lease")
            .backend();
        let other_session_backend = manager
            .ensure_session_backend(other_session)
            .await
            .expect("other session lease")
            .backend();
        let other_config = manager.ensure_session_backend(other_config).await;

        assert!(!Arc::ptr_eq(&base_backend, &other_session_backend));
        assert!(matches!(
            other_config,
            Err(OperationBackendBuildError::InvalidConfig { .. })
                | Err(OperationBackendBuildError::BuildFailed { .. })
        ));
    }

    #[tokio::test]
    async fn independent_backend_create_generates_backend_id_and_supports_get_list_delete() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let created = manager
            .create_backend(BackendCreateRequest {
                workspace_root: workspace.path().to_path_buf(),
                backend_id: None,
                provider: None,
                session_id: None,
                timeout: Some(60),
                metadata: json!({"user": "abc", "app": "prod"}),
                resource_limits: BackendResourceLimits::default(),
                options: Some(temp_options(&workspace)),
            })
            .await
            .expect("create managed backend");

        assert!(created.backend_id.starts_with("bkd_"));
        assert_eq!(created.session_id, None);
        assert!(created.expires_at_ms.is_some());

        let fetched = manager
            .get_backend(&created.backend_id)
            .await
            .expect("get managed backend");
        assert_eq!(fetched.backend_id, created.backend_id);

        let listed = manager
            .list_backends(BackendListFilter {
                metadata: BTreeMap::from([("user".to_string(), "abc".to_string())]),
            })
            .await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].backend_id, created.backend_id);

        manager
            .delete_backend(&created.backend_id)
            .await
            .expect("delete managed backend");
        assert!(matches!(
            manager.get_backend(&created.backend_id).await,
            Err(BackendError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn backend_create_allows_session_id_and_independent_backend_id() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let created = manager
            .create_backend(BackendCreateRequest {
                workspace_root: workspace.path().to_path_buf(),
                backend_id: Some("backend-a".to_string()),
                provider: None,
                session_id: Some("session-a".to_string()),
                timeout: None,
                metadata: Value::Null,
                resource_limits: BackendResourceLimits::default(),
                options: Some(temp_options(&workspace)),
            })
            .await
            .expect("create managed backend");

        assert_eq!(created.backend_id, "backend-a");
        assert_eq!(created.session_id.as_deref(), Some("session-a"));
        assert_eq!(created.session_ids, vec!["session-a".to_string()]);

        let lease = manager
            .ensure_session_backend(local_request(
                "session-a",
                workspace.path().to_path_buf(),
                temp_options(&workspace),
            ))
            .await
            .expect("session lease");
        assert_eq!(lease.instance().backend_id.0, "backend-a");
    }

    #[tokio::test]
    async fn connect_backend_explicitly_attaches_session_to_existing_backend() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let root = workspace.path().to_path_buf();
        let first = manager
            .ensure_session_backend(local_request("s1", root.clone(), temp_options(&workspace)))
            .await
            .expect("first lease");
        let backend_id = first.instance().backend_id.0;

        let connected = manager
            .connect_backend(
                &backend_id,
                BackendConnectRequest {
                    timeout: None,
                    session_id: Some("s2".to_string()),
                },
            )
            .await
            .expect("connect managed backend");
        assert_eq!(
            connected.session_ids,
            vec!["s1".to_string(), "s2".to_string()]
        );

        let second = manager
            .ensure_session_backend(local_request("s2", root, temp_options(&workspace)))
            .await
            .expect("second lease");
        assert_eq!(first.instance(), second.instance());
        assert!(Arc::ptr_eq(&first.backend(), &second.backend()));
    }

    #[tokio::test]
    async fn fork_backend_rejects_non_e2b_parent() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let parent = manager
            .ensure_session_backend(local_request(
                "s1",
                workspace.path().to_path_buf(),
                temp_options(&workspace),
            ))
            .await
            .expect("local backend");

        let forked = manager
            .fork_backend(BackendForkRequest {
                parent_backend_id: Some(parent.instance().backend_id.0),
                parent_session_id: Some("s1".to_string()),
                backend_id: Some("child".to_string()),
                session_id: Some("s2".to_string()),
                ..Default::default()
            })
            .await;

        assert!(matches!(
            forked,
            Err(BackendError::UnsupportedBackend { kind }) if kind == "local:checkout"
        ));
    }

    #[tokio::test]
    async fn clean_backend_checkpoint_reuses_previous_ref() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let lease = manager
            .ensure_session_backend(local_request(
                "s-checkpoint",
                workspace.path().to_path_buf(),
                temp_options(&workspace),
            ))
            .await
            .expect("local backend");

        let first = manager
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: Some(lease.instance().backend_id.0.clone()),
                session_id: Some("s-checkpoint".to_string()),
                name: Some("first".to_string()),
                metadata: json!({"step": 1}),
            })
            .await
            .expect("first checkpoint");
        let second = manager
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: Some(lease.instance().backend_id.0),
                session_id: Some("s-checkpoint".to_string()),
                name: Some("second".to_string()),
                metadata: json!({"step": 2}),
            })
            .await
            .expect("second checkpoint");

        assert!(!first.reused);
        assert!(second.reused);
        assert_eq!(
            second.checkpoint.checkpoint_id,
            first.checkpoint.checkpoint_id
        );
        assert_eq!(second.checkpoint.provider, "local");
        assert_eq!(second.checkpoint.provider_snapshot_id, None);
    }

    #[tokio::test]
    async fn write_and_exec_mark_backend_dirty_for_next_checkpoint() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let lease = manager
            .ensure_session_backend(local_request(
                "s-dirty",
                workspace.path().to_path_buf(),
                temp_options(&workspace),
            ))
            .await
            .expect("local backend");
        let backend_id = lease.instance().backend_id.0.clone();
        let baseline = manager
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: Some(backend_id.clone()),
                session_id: Some("s-dirty".to_string()),
                name: None,
                metadata: Value::Null,
            })
            .await
            .expect("baseline checkpoint");

        lease
            .backend()
            .files()
            .write_bytes(WriteBytesRequest {
                path: BackendPath(
                    workspace
                        .path()
                        .join("dirty.txt")
                        .to_string_lossy()
                        .to_string(),
                ),
                content: b"changed".to_vec(),
                mode: WriteMode::Overwrite,
            })
            .await
            .expect("write file");
        let after_write = manager
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: Some(backend_id.clone()),
                session_id: Some("s-dirty".to_string()),
                name: None,
                metadata: Value::Null,
            })
            .await
            .expect("checkpoint after write");

        assert!(!after_write.reused);
        assert_ne!(
            after_write.checkpoint.checkpoint_id,
            baseline.checkpoint.checkpoint_id
        );

        lease
            .backend()
            .exec()
            .exec(ExecRequest {
                command: "printf ok".to_string(),
                args: Vec::new(),
                shell: Some("/bin/sh".to_string()),
                cwd: Some(BackendPath(workspace.path().to_string_lossy().to_string())),
                timeout_ms: Some(5_000),
                env: None,
            })
            .await
            .expect("exec command");
        let after_exec = manager
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: Some(backend_id),
                session_id: Some("s-dirty".to_string()),
                name: None,
                metadata: Value::Null,
            })
            .await
            .expect("checkpoint after exec");

        assert!(!after_exec.reused);
        assert_ne!(
            after_exec.checkpoint.checkpoint_id,
            after_write.checkpoint.checkpoint_id
        );
    }

    #[tokio::test]
    async fn non_snapshot_provider_checkout_is_unsupported() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        manager
            .ensure_session_backend(local_request(
                "s-local-checkout",
                workspace.path().to_path_buf(),
                temp_options(&workspace),
            ))
            .await
            .expect("local backend");
        let checkpoint = manager
            .checkpoint_backend(BackendCheckpointRequest {
                backend_id: None,
                session_id: Some("s-local-checkout".to_string()),
                name: None,
                metadata: Value::Null,
            })
            .await
            .expect("checkpoint");

        let checkout = manager
            .checkout_backend(BackendCheckoutRequest {
                checkpoint: checkpoint.checkpoint,
                backend_id: None,
                session_id: Some("child".to_string()),
                timeout: None,
                metadata: Value::Null,
                resource_limits: BackendResourceLimits::default(),
                options: None,
            })
            .await;

        assert!(matches!(
            checkout,
            Err(BackendError::UnsupportedBackend { kind }) if kind == "local:checkout"
        ));
    }

    #[tokio::test]
    async fn list_backend_trees_uses_recorded_lineage() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let root = workspace.path().to_path_buf();
        let parent = manager
            .ensure_session_backend(local_request(
                "parent",
                root.clone(),
                temp_options(&workspace),
            ))
            .await
            .expect("parent backend")
            .instance()
            .backend_id;
        let child = manager
            .ensure_session_backend(local_request("child", root, temp_options(&workspace)))
            .await
            .expect("child backend")
            .instance()
            .backend_id;

        {
            let mut state = manager.state.lock().await;
            state
                .backends
                .get_mut(&parent)
                .expect("parent entry")
                .lineage
                .children_backend_ids
                .insert(child.0.clone(), ());
            let child_entry = state.backends.get_mut(&child).expect("child entry");
            child_entry.lineage.parent_backend_id = Some(parent.clone());
            child_entry.lineage.forked_from_snapshot_id = Some("snap:default".to_string());
        }

        let forest = manager.list_backend_trees().await;
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].backend.backend_id, parent.0);
        assert_eq!(forest[0].children.len(), 1);
        assert_eq!(forest[0].children[0].backend.backend_id, child.0);
        assert_eq!(
            forest[0].children[0]
                .backend
                .lineage
                .forked_from_snapshot_id
                .as_deref(),
            Some("snap:default")
        );
    }

    #[tokio::test]
    async fn release_session_deletes_local_backend_cache() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let request = local_request(
            "s1",
            workspace.path().to_path_buf(),
            temp_options(&workspace),
        );

        let first_backend: Arc<_> = manager
            .ensure_session_backend(request.clone())
            .await
            .expect("first lease")
            .backend();
        manager.release_session("s1").await.expect("release");
        let second_backend = manager
            .ensure_session_backend(request)
            .await
            .expect("second lease")
            .backend();

        assert!(!Arc::ptr_eq(&first_backend, &second_backend));
    }

    #[tokio::test]
    async fn local_provider_delete_succeeds_and_pause_is_unsupported() {
        let workspace = TempDir::new().expect("workspace");
        let provider = operation_backend::local_backend_provider();
        let lease = BackendManager::new()
            .ensure_session_backend(local_request(
                "s1",
                workspace.path().to_path_buf(),
                temp_options(&workspace),
            ))
            .await
            .expect("local backend");
        let instance = lease.instance();

        let pause = provider
            .pause(BackendPauseRequest {
                backend_id: instance.backend_id.clone(),
                instance_id: instance.instance_id.clone(),
                mode: BackendPauseMode::BestEffort,
                reason: BackendLifecycleReason::UserRequested,
                metadata: Value::Null,
            })
            .await;
        assert!(matches!(
            pause,
            Err(agent_contracts::backend::BackendControlError::UnsupportedCapability { .. })
        ));

        let outcome = provider
            .delete(BackendDeleteRequest {
                backend_id: instance.backend_id.clone(),
                instance_id: Some(instance.instance_id.clone()),
                snapshot_id: None,
                force: false,
                reason: BackendLifecycleReason::SessionClose,
                metadata: instance.metadata.clone(),
            })
            .await
            .expect("delete");

        assert_eq!(outcome.backend_id, instance.backend_id);
        assert_eq!(outcome.instance_id, Some(instance.instance_id));
        assert_eq!(outcome.state, BackendLifecycleState::Deleted);
    }
}
