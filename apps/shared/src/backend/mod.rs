use agent_contracts::backend::{
    BackendControlError, BackendCreateRequest, BackendDeleteRequest, BackendEndpoint, BackendId,
    BackendInstance, BackendLifecycle, BackendLifecycleReason, BackendLifecycleState, BackendPath,
    BackendProvider, BackendResourceAllocation, BackendResourceLimits, OperationBackend,
    OperationBackendBuildError, OperationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayBackendConfig {
    pub kind: String,
    pub options: Value,
}

impl GatewayBackendConfig {
    pub fn new(kind: impl Into<String>, options: Value) -> Self {
        Self {
            kind: kind.into(),
            options,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackendEnsureSessionRequest {
    pub config: Option<GatewayBackendConfig>,
    pub workspace_root: PathBuf,
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxCreateRequest {
    pub workspace_root: PathBuf,
    #[serde(default)]
    pub backend_id: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub resource_limits: BackendResourceLimits,
    #[serde(default)]
    pub options: Option<Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SandboxConnectRequest {
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct SandboxListFilter {
    pub metadata: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxInfo {
    pub backend_id: String,
    pub provider: String,
    pub instance_id: String,
    pub state: BackendLifecycleState,
    pub workspace_root: String,
    pub endpoint: Option<BackendEndpoint>,
    pub metadata: Value,
    pub resources: BackendResourceAllocation,
    pub session_id: Option<String>,
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("invalid sandbox request: {message}")]
    InvalidRequest { message: String },
    #[error("sandbox conflict: {message}")]
    Conflict { message: String },
    #[error("sandbox not found: {backend_id}")]
    NotFound { backend_id: String },
    #[error("unsupported backend kind: {kind}")]
    UnsupportedBackend { kind: String },
    #[error("sandbox backend build failed: {message}")]
    BuildFailed { message: String },
    #[error("sandbox backend operation failed: {message}")]
    Operation { message: String },
}

#[derive(Clone)]
pub struct BackendLease {
    backend: Arc<dyn OperationBackend>,
    instance: BackendInstance,
}

impl BackendLease {
    pub fn backend(&self) -> Arc<dyn OperationBackend> {
        Arc::clone(&self.backend)
    }

    pub fn instance(&self) -> BackendInstance {
        self.instance.clone()
    }
}

struct BackendInstanceEntry {
    backend: Arc<dyn OperationBackend>,
    instance: BackendInstance,
    config: GatewayBackendConfig,
    workspace_root: String,
    config_hash: u64,
    session_id: Option<String>,
    expires_at_ms: Option<u64>,
}

#[derive(Default)]
pub struct BackendManager {
    state: Mutex<BackendManagerState>,
}

#[derive(Default)]
struct BackendManagerState {
    sandboxes: HashMap<BackendId, BackendInstanceEntry>,
    session_index: HashMap<String, BackendId>,
}

impl BackendManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn create_sandbox(
        &self,
        request: SandboxCreateRequest,
    ) -> Result<SandboxInfo, SandboxError> {
        let config = resolve_sandbox_backend_config(request.provider.clone(), request.options)?;
        let backend_id = requested_backend_id(request.backend_id, request.session_id.as_deref())?;
        let workspace_root = workspace_root_string(&request.workspace_root)
            .map_err(SandboxError::from_build_error)?;
        let config_hash = hash_config(&config);
        let expires_at_ms = request.timeout.map(expires_at_ms_from_timeout);

        let mut state = self.state.lock().await;
        if state.sandboxes.contains_key(&backend_id) {
            return Err(SandboxError::Conflict {
                message: format!("backend_id {backend_id} already exists"),
            });
        }
        if let Some(session_id) = request.session_id.as_ref() {
            if let Some(existing_backend_id) = state.session_index.get(session_id) {
                return Err(SandboxError::Conflict {
                    message: format!(
                        "session_id {session_id} is already bound to backend_id {existing_backend_id}"
                    ),
                });
            }
        }

        let entry = build_managed_backend(BuildSandboxInput {
            backend_id: backend_id.clone(),
            config,
            workspace_root_text: workspace_root,
            config_hash,
            session_id_for_instance: request
                .session_id
                .clone()
                .unwrap_or_else(|| backend_id.0.clone()),
            session_id: request.session_id,
            resource_limits: request.resource_limits,
            metadata: request.metadata,
            expires_at_ms,
        })
        .await?;
        let info = entry.info();
        if let Some(session_id) = entry.session_id.as_ref() {
            state
                .session_index
                .insert(session_id.clone(), backend_id.clone());
        }
        state.sandboxes.insert(backend_id, entry);
        Ok(info)
    }

    pub async fn connect_sandbox(
        &self,
        backend_id: &str,
        request: SandboxConnectRequest,
    ) -> Result<SandboxInfo, SandboxError> {
        let mut state = self.state.lock().await;
        let entry = state
            .sandboxes
            .get_mut(&BackendId(backend_id.to_string()))
            .ok_or_else(|| SandboxError::NotFound {
                backend_id: backend_id.to_string(),
            })?;
        if let Some(timeout) = request.timeout {
            entry.expires_at_ms = Some(expires_at_ms_from_timeout(timeout));
        }
        Ok(entry.info())
    }

    pub async fn get_sandbox(&self, backend_id: &str) -> Result<SandboxInfo, SandboxError> {
        let state = self.state.lock().await;
        state
            .sandboxes
            .get(&BackendId(backend_id.to_string()))
            .map(BackendInstanceEntry::info)
            .ok_or_else(|| SandboxError::NotFound {
                backend_id: backend_id.to_string(),
            })
    }

    pub async fn list_sandboxes(&self, filter: SandboxListFilter) -> Vec<SandboxInfo> {
        let state = self.state.lock().await;
        state
            .sandboxes
            .values()
            .filter(|entry| metadata_matches_filter(&entry.instance.metadata, &filter.metadata))
            .map(BackendInstanceEntry::info)
            .collect()
    }

    pub async fn delete_sandbox(&self, backend_id: &str) -> Result<(), SandboxError> {
        let removed = {
            let mut state = self.state.lock().await;
            let backend_id = BackendId(backend_id.to_string());
            let entry =
                state
                    .sandboxes
                    .remove(&backend_id)
                    .ok_or_else(|| SandboxError::NotFound {
                        backend_id: backend_id.0.clone(),
                    })?;
            if let Some(session_id) = entry.session_id.as_ref() {
                state.session_index.remove(session_id);
            }
            entry
        };
        delete_backend_instance(removed, BackendLifecycleReason::UserRequested)
            .await
            .map_err(SandboxError::from_operation_error)
    }

    pub async fn ensure_session_backend(
        &self,
        request: BackendEnsureSessionRequest,
    ) -> Result<BackendLease, OperationBackendBuildError> {
        let config = resolve_session_backend_config(request.config.clone())?;
        let backend_id = BackendId(request.session_id.clone());
        let workspace_root = workspace_root_string(&request.workspace_root)?;
        let config_hash = hash_config(&config);
        let mut state = self.state.lock().await;

        if let Some(existing_backend_id) = state.session_index.get(&request.session_id) {
            if existing_backend_id != &backend_id {
                return Err(OperationBackendBuildError::BuildFailed {
                    message: format!(
                        "session {} is already bound to backend {}",
                        request.session_id, existing_backend_id
                    ),
                });
            }
        }

        if let Some(entry) = state.sandboxes.get(&backend_id) {
            if entry.workspace_root != workspace_root || entry.config_hash != config_hash {
                return Err(OperationBackendBuildError::BuildFailed {
                    message: format!(
                        "session {} is already bound to backend {} with different workspace or config",
                        request.session_id, backend_id
                    ),
                });
            }
            return Ok(BackendLease {
                backend: Arc::clone(&entry.backend),
                instance: entry.instance.clone(),
            });
        }

        let entry = build_managed_backend(BuildSandboxInput {
            backend_id: backend_id.clone(),
            config,
            workspace_root_text: workspace_root,
            config_hash,
            session_id_for_instance: request.session_id.clone(),
            session_id: Some(request.session_id.clone()),
            resource_limits: BackendResourceLimits::default(),
            metadata: Value::Null,
            expires_at_ms: None,
        })
        .await
        .map_err(SandboxError::into_build_error)?;
        let backend = Arc::clone(&entry.backend);
        let instance = entry.instance.clone();

        state
            .session_index
            .insert(request.session_id, backend_id.clone());
        state.sandboxes.insert(backend_id, entry);
        Ok(BackendLease { backend, instance })
    }

    pub async fn release_session(&self, session_id: &str) -> Result<(), OperationError> {
        let removed = {
            let mut state = self.state.lock().await;
            state
                .session_index
                .remove(session_id)
                .and_then(|backend_id| state.sandboxes.remove(&backend_id))
        };

        if let Some(instance) = removed {
            delete_backend_instance(instance, BackendLifecycleReason::SessionClose).await?;
        }
        Ok(())
    }

    pub async fn shutdown_all(&self) -> Result<(), OperationError> {
        let removed = {
            let mut state = self.state.lock().await;
            state.session_index.clear();
            state
                .sandboxes
                .drain()
                .map(|(_, instance)| instance)
                .collect::<Vec<_>>()
        };

        for instance in removed {
            delete_backend_instance(instance, BackendLifecycleReason::DaemonShutdown).await?;
        }
        Ok(())
    }
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
        Some(config) if config.kind == "local" => Ok(config),
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

fn requested_backend_id(
    backend_id: Option<String>,
    session_id: Option<&str>,
) -> Result<BackendId, SandboxError> {
    match (backend_id, session_id) {
        (Some(backend_id), Some(session_id)) if backend_id != session_id => {
            Err(SandboxError::InvalidRequest {
                message: "backend_id must match session_id when both are provided".to_string(),
            })
        }
        (Some(backend_id), _) => Ok(BackendId(backend_id)),
        (None, Some(session_id)) => Ok(BackendId(session_id.to_string())),
        (None, None) => Ok(BackendId(format!("sbx_{}", uuid::Uuid::new_v4().simple()))),
    }
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
}

async fn build_managed_backend(
    input: BuildSandboxInput,
) -> Result<BackendInstanceEntry, SandboxError> {
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
        session_id: input.session_id,
        expires_at_ms: input.expires_at_ms,
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
        SandboxInfo {
            backend_id: self.instance.backend_id.0.clone(),
            provider: self.instance.provider.0.clone(),
            instance_id: self.instance.instance_id.0.clone(),
            state: self.instance.state,
            workspace_root: self.instance.workspace_root.0.clone(),
            endpoint: self.instance.endpoint.clone(),
            metadata: self.instance.metadata.clone(),
            resources: self.instance.resources,
            session_id: self.session_id.clone(),
            expires_at_ms: self.expires_at_ms,
        }
    }
}

impl SandboxError {
    fn from_build_error(error: OperationBackendBuildError) -> Self {
        match error {
            OperationBackendBuildError::InvalidConfig { message } => {
                Self::InvalidRequest { message }
            }
            OperationBackendBuildError::UnsupportedBackend { kind } => {
                Self::UnsupportedBackend { kind }
            }
            OperationBackendBuildError::Unsupported { message }
            | OperationBackendBuildError::BuildFailed { message } => Self::BuildFailed { message },
        }
    }

    fn from_control_error(error: BackendControlError) -> Self {
        match error {
            BackendControlError::InvalidRequest { message } => Self::InvalidRequest { message },
            BackendControlError::UnsupportedCapability {
                provider,
                capability,
            } => Self::UnsupportedBackend {
                kind: format!("{}:{capability}", provider.0),
            },
            BackendControlError::NotFound { id, .. } => Self::NotFound { backend_id: id },
            BackendControlError::InvalidState { message, .. } => Self::Conflict { message },
            BackendControlError::ProviderError { message, .. }
            | BackendControlError::Transport { message } => Self::BuildFailed { message },
            BackendControlError::Timeout {
                operation,
                timeout_ms,
            } => Self::BuildFailed {
                message: format!("{operation} timed out after {timeout_ms} ms"),
            },
        }
    }

    fn from_operation_error(error: OperationError) -> Self {
        Self::Operation {
            message: error.to_string(),
        }
    }

    fn into_build_error(self) -> OperationBackendBuildError {
        match self {
            Self::InvalidRequest { message } | Self::Conflict { message } => {
                OperationBackendBuildError::InvalidConfig { message }
            }
            Self::UnsupportedBackend { kind } => {
                OperationBackendBuildError::UnsupportedBackend { kind }
            }
            Self::NotFound { backend_id } => OperationBackendBuildError::BuildFailed {
                message: format!("sandbox not found: {backend_id}"),
            },
            Self::BuildFailed { message } | Self::Operation { message } => {
                OperationBackendBuildError::BuildFailed { message }
            }
        }
    }

    fn into_operation_error(self) -> OperationError {
        OperationError::Transport {
            message: self.to_string(),
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
        assert_eq!(first.instance().backend_id.0, "s1");
        assert!(Arc::ptr_eq(&first_backend, &second_backend));
    }

    #[tokio::test]
    async fn manager_splits_backends_by_session_but_rejects_same_session_config_drift() {
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
    async fn sandbox_create_requires_matching_session_and_backend_ids() {
        let workspace = TempDir::new().expect("workspace");
        let manager = BackendManager::new();
        let result = manager
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
            .await;

        assert!(matches!(result, Err(SandboxError::InvalidRequest { .. })));
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

        let first_backend = manager
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
