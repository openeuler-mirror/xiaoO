use agent_contracts::backend::{
    BackendCreateRequest, BackendDeleteRequest, BackendInstance, BackendLifecycle,
    BackendLifecycleReason, BackendPath, BackendProvider, BackendResourceLimits, OperationBackend,
    OperationBackendBuildError, OperationError,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BackendInstanceKey {
    session_id: String,
    workspace_root: String,
    config_hash: u64,
}

struct BackendInstanceEntry {
    backend: Arc<dyn OperationBackend>,
    instance: BackendInstance,
}

#[derive(Default)]
pub struct ExternalBackendManager {
    instances: Mutex<HashMap<BackendInstanceKey, BackendInstanceEntry>>,
}

pub type BackendManager = ExternalBackendManager;

impl ExternalBackendManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn ensure_session_backend(
        &self,
        request: BackendEnsureSessionRequest,
    ) -> Result<BackendLease, OperationBackendBuildError> {
        let config = resolve_session_backend_config(request.config.clone())?;
        let key = BackendInstanceKey::from_request(&request, &config)?;
        let mut instances = self.instances.lock().await;
        if let Some(entry) = instances.get(&key) {
            return Ok(BackendLease {
                backend: Arc::clone(&entry.backend),
                instance: entry.instance.clone(),
            });
        }

        let (backend, instance) = build_local_managed_backend(&request, &config).await?;

        instances.insert(
            key,
            BackendInstanceEntry {
                backend: Arc::clone(&backend),
                instance: instance.clone(),
            },
        );
        Ok(BackendLease { backend, instance })
    }

    pub async fn release_session(&self, session_id: &str) -> Result<(), OperationError> {
        let removed = {
            let mut instances = self.instances.lock().await;
            let keys: Vec<_> = instances
                .keys()
                .filter(|key| key.session_id == session_id)
                .cloned()
                .collect();
            keys.into_iter()
                .filter_map(|key| instances.remove(&key))
                .collect::<Vec<_>>()
        };

        for instance in removed {
            delete_backend_instance(instance).await?;
        }
        Ok(())
    }

    pub async fn shutdown_all(&self) -> Result<(), OperationError> {
        let removed = {
            let mut instances = self.instances.lock().await;
            instances
                .drain()
                .map(|(_, instance)| instance)
                .collect::<Vec<_>>()
        };

        for instance in removed {
            delete_backend_instance(instance).await?;
        }
        Ok(())
    }
}

impl BackendInstanceKey {
    fn from_request(
        request: &BackendEnsureSessionRequest,
        config: &GatewayBackendConfig,
    ) -> Result<Self, OperationBackendBuildError> {
        let workspace_root = workspace_root_string(&request.workspace_root)?;
        Ok(Self {
            session_id: request.session_id.clone(),
            workspace_root,
            config_hash: hash_config(&config),
        })
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

fn backend_path_from_workspace(path: &PathBuf) -> Result<BackendPath, OperationBackendBuildError> {
    workspace_root_string(path).map(BackendPath)
}

async fn build_local_managed_backend(
    request: &BackendEnsureSessionRequest,
    config: &GatewayBackendConfig,
) -> Result<(Arc<dyn OperationBackend>, BackendInstance), OperationBackendBuildError> {
    let provider = operation_backend::local_backend_provider();
    let workspace_root = backend_path_from_workspace(&request.workspace_root)?;
    let lifecycle = provider.lifecycle();
    let instance = lifecycle
        .create_sandbox(BackendCreateRequest {
            session_id: request.session_id.clone(),
            conversation_id: None,
            workspace_root,
            provider_options: config.options.clone(),
            resource_limits: BackendResourceLimits::default(),
            metadata: Value::Null,
        })
        .await
        .map_err(control_error_to_build_error)?;
    let backend = provider
        .attach(instance.clone())
        .await
        .map_err(control_error_to_build_error)?;
    Ok((backend, instance))
}

async fn delete_backend_instance(instance: BackendInstanceEntry) -> Result<(), OperationError> {
    let provider = operation_backend::local_backend_provider();
    provider
        .delete(BackendDeleteRequest {
            backend_id: instance.instance.backend_id,
            instance_id: Some(instance.instance.instance_id),
            snapshot_id: instance
                .instance
                .snapshot
                .map(|snapshot| snapshot.snapshot_id),
            force: false,
            reason: BackendLifecycleReason::SessionClose,
            metadata: instance.instance.metadata,
        })
        .await
        .map_err(control_error_to_operation_error)?;
    Ok(())
}

fn control_error_to_build_error(
    error: agent_contracts::backend::BackendControlError,
) -> OperationBackendBuildError {
    OperationBackendBuildError::BuildFailed {
        message: error.to_string(),
    }
}

fn control_error_to_operation_error(
    error: agent_contracts::backend::BackendControlError,
) -> OperationError {
    OperationError::Transport {
        message: error.to_string(),
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

    fn local_key(request: &BackendEnsureSessionRequest) -> BackendInstanceKey {
        let config = resolve_session_backend_config(request.config.clone()).expect("config");
        BackendInstanceKey::from_request(request, &config).expect("key")
    }

    #[test]
    fn backend_key_is_stable_for_reordered_config() {
        let left = local_key(&local_request(
            "s1",
            PathBuf::from("/workspace"),
            json!({"home_dir": "/home/user", "temp_root": "/tmp/xiaoo"}),
        ));
        let right = local_key(&local_request(
            "s1",
            PathBuf::from("/workspace"),
            json!({"temp_root": "/tmp/xiaoo", "home_dir": "/home/user"}),
        ));

        assert_eq!(left, right);
    }

    #[test]
    fn backend_key_splits_by_session_and_config() {
        let base = local_key(&local_request(
            "s1",
            PathBuf::from("/workspace"),
            json!({"temp_root": "/tmp/xiaoo"}),
        ));
        let other_session = local_key(&local_request(
            "s2",
            PathBuf::from("/workspace"),
            json!({"temp_root": "/tmp/xiaoo"}),
        ));
        let other_config = local_key(&local_request(
            "s1",
            PathBuf::from("/workspace"),
            json!({"temp_root": "/tmp/other"}),
        ));

        assert_ne!(base, other_session);
        assert_ne!(base, other_config);
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
        let manager = ExternalBackendManager::new();
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
        assert!(Arc::ptr_eq(&first_backend, &second_backend));
    }

    #[tokio::test]
    async fn manager_splits_backend_by_session_and_config_hash() {
        let workspace = TempDir::new().expect("workspace");
        let manager = ExternalBackendManager::new();
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
        let other_config_backend = manager
            .ensure_session_backend(other_config)
            .await
            .expect("other config lease")
            .backend();

        assert!(!Arc::ptr_eq(&base_backend, &other_session_backend));
        assert!(!Arc::ptr_eq(&base_backend, &other_config_backend));
    }

    #[tokio::test]
    async fn release_session_deletes_local_backend_cache() {
        let workspace = TempDir::new().expect("workspace");
        let manager = ExternalBackendManager::new();
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
        let lease_request = local_request(
            "s1",
            workspace.path().to_path_buf(),
            temp_options(&workspace),
        );
        let (_backend, instance) = build_local_managed_backend(
            &lease_request,
            &resolve_session_backend_config(lease_request.config.clone()).expect("config"),
        )
        .await
        .expect("local backend");

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
