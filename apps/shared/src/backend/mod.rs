use agent_contracts::backend::{
    BackendControlError, BackendCreateRequest, BackendDeleteRequest, BackendId, BackendInstance,
    BackendLifecycle, BackendLifecycleReason, BackendPath, BackendProvider, BackendResourceLimits,
    OperationBackend, OperationBackendBuildError, OperationError,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;

mod backend_manager;
mod base;
mod e2b;

pub use backend_manager::BackendManager;
use backend_manager::BackendManagerState;
pub use base::{
    BackendEnsureSessionRequest, BackendLease, GatewayBackendConfig, SandboxConnectRequest,
    SandboxCreateRequest, SandboxError, SandboxForkRequest, SandboxForkResult, SandboxInfo,
    SandboxLineageInfo, SandboxListFilter, SandboxTreeNode,
};

struct BackendInstanceEntry {
    backend: Arc<dyn OperationBackend>,
    instance: BackendInstance,
    config: GatewayBackendConfig,
    workspace_root: String,
    config_hash: u64,
    session_ids: BTreeMap<String, ()>,
    expires_at_ms: Option<u64>,
    lineage: SandboxLineageEntry,
}

#[derive(Debug, Clone, Default)]
struct SandboxLineageEntry {
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

fn resolve_sandbox_backend_config(
    provider: Option<String>,
    options: Option<Value>,
) -> Result<GatewayBackendConfig, SandboxError> {
    let kind = provider.unwrap_or_else(|| "local".to_string());
    let options = options.unwrap_or_else(|| {
        if kind == "local" {
            default_local_provider_options()
        } else {
            Value::Object(Map::new())
        }
    });
    resolve_session_backend_config(Some(GatewayBackendConfig::new(kind, options)))
        .map_err(SandboxError::from_build_error)
}

fn requested_backend_id(backend_id: Option<String>) -> Result<BackendId, SandboxError> {
    match backend_id {
        Some(backend_id) if backend_id.trim().is_empty() => Err(SandboxError::InvalidRequest {
            message: "backend_id cannot be empty".to_string(),
        }),
        Some(backend_id) => Ok(BackendId(backend_id)),
        None => Ok(new_backend_id()),
    }
}

fn new_backend_id() -> BackendId {
    BackendId(format!("sbx_{}", uuid::Uuid::new_v4().simple()))
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

fn resolve_parent_backend_id(
    state: &BackendManagerState,
    request: &SandboxForkRequest,
) -> Result<BackendId, SandboxError> {
    let by_backend = request
        .parent_backend_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| BackendId(value.to_string()));
    let by_session = request
        .parent_session_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|session_id| {
            state
                .session_index
                .get(session_id)
                .cloned()
                .ok_or_else(|| SandboxError::NotFound {
                    backend_id: format!("session:{session_id}"),
                })
        })
        .transpose()?;

    match (by_backend, by_session) {
        (Some(backend_id), Some(session_backend_id)) if backend_id != session_backend_id => {
            Err(SandboxError::Conflict {
                message: format!(
                    "parent_backend_id {backend_id} does not match parent_session_id backend {session_backend_id}"
                ),
            })
        }
        (Some(backend_id), _) | (_, Some(backend_id)) => Ok(backend_id),
        (None, None) => Err(SandboxError::InvalidRequest {
            message: "fork requires parent_backend_id or parent_session_id".to_string(),
        }),
    }
}

struct ParentForkSource {
    backend_id: BackendId,
    config: GatewayBackendConfig,
    workspace_root: String,
    instance_id: String,
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

fn fork_metadata(
    metadata: Value,
    parent: &ParentForkSource,
    child_backend_id: &str,
    snapshot_id: &str,
) -> Value {
    let mut object = match metadata {
        Value::Object(object) => object,
        Value::Null => Map::new(),
        other => {
            let mut object = Map::new();
            object.insert("user_metadata".to_string(), other);
            object
        }
    };
    object.insert(
        "xiaoo_fork_parent_backend_id".to_string(),
        Value::String(parent.backend_id.0.clone()),
    );
    object.insert(
        "xiaoo_fork_parent_sandbox_id".to_string(),
        Value::String(parent.instance_id.clone()),
    );
    object.insert(
        "xiaoo_fork_snapshot_id".to_string(),
        Value::String(snapshot_id.to_string()),
    );
    object.insert(
        "xiaoo_fork_child_backend_id".to_string(),
        Value::String(child_backend_id.to_string()),
    );
    Value::Object(object)
}

fn insert_forked_child(
    state: &mut BackendManagerState,
    parent_backend_id: &BackendId,
    child_backend_id: &BackendId,
    child_session_id: Option<String>,
    child_entry: BackendInstanceEntry,
) -> Result<SandboxInfo, SandboxError> {
    let parent_entry =
        state
            .sandboxes
            .get_mut(parent_backend_id)
            .ok_or_else(|| SandboxError::NotFound {
                backend_id: parent_backend_id.0.clone(),
            })?;
    parent_entry
        .lineage
        .children_backend_ids
        .insert(child_backend_id.0.clone(), ());
    let parent_info = parent_entry.info();
    if let Some(session_id) = child_session_id {
        state
            .session_index
            .insert(session_id, child_backend_id.clone());
    }
    state
        .sandboxes
        .insert(child_backend_id.clone(), child_entry);
    Ok(parent_info)
}

fn detach_from_parent(
    state: &mut BackendManagerState,
    backend_id: &BackendId,
    entry: &BackendInstanceEntry,
) {
    let Some(parent_backend_id) = entry.lineage.parent_backend_id.as_ref() else {
        return;
    };
    if let Some(parent) = state.sandboxes.get_mut(parent_backend_id) {
        parent.lineage.children_backend_ids.remove(&backend_id.0);
    }
}

fn sandbox_tree_node(
    state: &BackendManagerState,
    backend_id: &BackendId,
) -> Option<SandboxTreeNode> {
    let entry = state.sandboxes.get(backend_id)?;
    let mut child_ids = entry
        .lineage
        .children_backend_ids
        .keys()
        .filter_map(|id| {
            state
                .sandboxes
                .contains_key(&BackendId(id.clone()))
                .then(|| id.clone())
        })
        .collect::<Vec<_>>();
    child_ids.sort();
    let children = child_ids
        .into_iter()
        .filter_map(|id| sandbox_tree_node(state, &BackendId(id)))
        .collect();
    Some(SandboxTreeNode {
        sandbox: entry.info(),
        children,
    })
}

struct BuildSandboxInput {
    backend_id: BackendId,
    config: GatewayBackendConfig,
    workspace_root_text: String,
    config_hash: u64,
    session_id_for_instance: String,
    session_id: Option<String>,
    resource_limits: BackendResourceLimits,
    metadata: Value,
    expires_at_ms: Option<u64>,
    lineage: SandboxLineageEntry,
}

async fn build_managed_backend(
    input: BuildSandboxInput,
) -> Result<BackendInstanceEntry, SandboxError> {
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
        return Ok(BackendInstanceEntry {
            backend: created.backend,
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
        });
    }

    let provider = local_provider_for_kind(&input.config.kind)?;
    let lifecycle = provider.lifecycle();
    let instance = lifecycle
        .create_sandbox(BackendCreateRequest {
            requested_backend_id: Some(input.backend_id),
            session_id: input.session_id_for_instance,
            conversation_id: None,
            workspace_root: BackendPath(input.workspace_root_text.clone()),
            provider_options: input.config.options.clone(),
            resource_limits: input.resource_limits,
            metadata: input.metadata,
        })
        .await
        .map_err(SandboxError::from_control_error)?;
    let backend = provider
        .attach(instance.clone())
        .await
        .map_err(SandboxError::from_control_error)?;
    Ok(BackendInstanceEntry {
        backend,
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
    })
}

fn local_provider_for_kind(
    kind: &str,
) -> Result<operation_backend::LocalBackendProvider, SandboxError> {
    match kind {
        "local" => Ok(operation_backend::local_backend_provider()),
        other => Err(SandboxError::UnsupportedBackend {
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
        .map_err(SandboxError::into_operation_error)?;
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
    fn info(&self) -> SandboxInfo {
        let session_ids = self.session_ids.keys().cloned().collect::<Vec<_>>();
        SandboxInfo {
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
            lineage: SandboxLineageInfo {
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
        BackendLifecycle, BackendLifecycleState, BackendPauseMode, BackendPauseRequest,
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
        assert!(first.instance().backend_id.0.starts_with("sbx_"));
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
    async fn independent_sandbox_create_generates_backend_id_and_supports_get_list_delete() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let created = manager
            .create_sandbox(SandboxCreateRequest {
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
            .expect("create sandbox");

        assert!(created.backend_id.starts_with("sbx_"));
        assert_eq!(created.session_id, None);
        assert!(created.expires_at_ms.is_some());

        let fetched = manager
            .get_sandbox(&created.backend_id)
            .await
            .expect("get sandbox");
        assert_eq!(fetched.backend_id, created.backend_id);

        let listed = manager
            .list_sandboxes(SandboxListFilter {
                metadata: BTreeMap::from([("user".to_string(), "abc".to_string())]),
            })
            .await;
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].backend_id, created.backend_id);

        manager
            .delete_sandbox(&created.backend_id)
            .await
            .expect("delete sandbox");
        assert!(matches!(
            manager.get_sandbox(&created.backend_id).await,
            Err(SandboxError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn sandbox_create_allows_session_id_and_independent_backend_id() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let created = manager
            .create_sandbox(SandboxCreateRequest {
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
            .expect("create sandbox");

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
    async fn connect_sandbox_explicitly_attaches_session_to_existing_backend() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let root = workspace.path().to_path_buf();
        let first = manager
            .ensure_session_backend(local_request("s1", root.clone(), temp_options(&workspace)))
            .await
            .expect("first lease");
        let backend_id = first.instance().backend_id.0;

        let connected = manager
            .connect_sandbox(
                &backend_id,
                SandboxConnectRequest {
                    timeout: None,
                    session_id: Some("s2".to_string()),
                },
            )
            .await
            .expect("connect sandbox");
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
    async fn fork_sandbox_rejects_non_e2b_parent() {
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
            .fork_sandbox(SandboxForkRequest {
                parent_backend_id: Some(parent.instance().backend_id.0),
                parent_session_id: Some("s1".to_string()),
                backend_id: Some("child".to_string()),
                session_id: Some("s2".to_string()),
                ..Default::default()
            })
            .await;

        assert!(matches!(
            forked,
            Err(SandboxError::UnsupportedBackend { kind }) if kind == "local:fork"
        ));
    }

    #[tokio::test]
    async fn list_sandbox_trees_uses_recorded_lineage() {
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
                .sandboxes
                .get_mut(&parent)
                .expect("parent entry")
                .lineage
                .children_backend_ids
                .insert(child.0.clone(), ());
            let child_entry = state.sandboxes.get_mut(&child).expect("child entry");
            child_entry.lineage.parent_backend_id = Some(parent.clone());
            child_entry.lineage.forked_from_snapshot_id = Some("snap:default".to_string());
        }

        let forest = manager.list_sandbox_trees().await;
        assert_eq!(forest.len(), 1);
        assert_eq!(forest[0].sandbox.backend_id, parent.0);
        assert_eq!(forest[0].children.len(), 1);
        assert_eq!(forest[0].children[0].sandbox.backend_id, child.0);
        assert_eq!(
            forest[0].children[0]
                .sandbox
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
