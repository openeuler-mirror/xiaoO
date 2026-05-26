pub mod conch;

use agent_contracts::backend::{
    OperationBackend, OperationBackendBuildError, OperationBackendConfig, OperationError,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendScope {
    Session,
}

#[derive(Debug, Clone)]
pub struct BackendAcquireRequest {
    pub config: GatewayBackendConfig,
    pub scope: BackendScope,
    pub workspace_root: PathBuf,
    pub session_id: String,
}

#[derive(Clone)]
pub struct BackendLease {
    backend: Arc<dyn OperationBackend>,
}

impl BackendLease {
    pub fn backend(&self) -> Arc<dyn OperationBackend> {
        Arc::clone(&self.backend)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BackendInstanceKey {
    scope: &'static str,
    session_id: String,
    workspace_root: String,
    config_hash: u64,
}

struct BackendInstance {
    backend: Arc<dyn OperationBackend>,
}

#[derive(Default)]
pub struct ExternalBackendManager {
    instances: Mutex<HashMap<BackendInstanceKey, BackendInstance>>,
}

impl ExternalBackendManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn acquire(
        &self,
        request: BackendAcquireRequest,
    ) -> Result<BackendLease, OperationBackendBuildError> {
        let key = BackendInstanceKey::from_request(&request)?;
        let mut instances = self.instances.lock().await;
        if let Some(instance) = instances.get(&key) {
            return Ok(BackendLease {
                backend: Arc::clone(&instance.backend),
            });
        }

        let backend = match request.config.kind.as_str() {
            "conch" => {
                let config = operation_config_for_acquire(&request)?;
                conch::build_backend(&config).await?
            }
            other => {
                return Err(OperationBackendBuildError::UnsupportedBackend {
                    kind: other.to_string(),
                });
            }
        };

        instances.insert(
            key,
            BackendInstance {
                backend: Arc::clone(&backend),
            },
        );
        Ok(BackendLease { backend })
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
            instance.backend.shutdown().await?;
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
            instance.backend.shutdown().await?;
        }
        Ok(())
    }
}

fn operation_config_for_acquire(
    request: &BackendAcquireRequest,
) -> Result<OperationBackendConfig, OperationBackendBuildError> {
    let mut options = request.config.options.clone();
    if request.config.kind == "conch" {
        let workspace_root = workspace_root_string(&request.workspace_root)?;
        let Some(options) = options.as_object_mut() else {
            return Err(OperationBackendBuildError::InvalidConfig {
                message: "conch backend options must be a table/object".to_string(),
            });
        };
        options.insert("workspace_root".to_string(), Value::String(workspace_root));
    }

    Ok(OperationBackendConfig::new(
        request.config.kind.clone(),
        options,
    ))
}

impl BackendInstanceKey {
    fn from_request(request: &BackendAcquireRequest) -> Result<Self, OperationBackendBuildError> {
        let workspace_root = workspace_root_string(&request.workspace_root)?;
        let scope = match request.scope {
            BackendScope::Session => "session",
        };
        Ok(Self {
            scope,
            session_id: request.session_id.clone(),
            workspace_root: workspace_root.to_string(),
            config_hash: hash_config(&request.config),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(session_id: &str, options: Value) -> BackendAcquireRequest {
        BackendAcquireRequest {
            config: GatewayBackendConfig::new("conch", options),
            scope: BackendScope::Session,
            workspace_root: PathBuf::from("/workspace"),
            session_id: session_id.to_string(),
        }
    }

    #[test]
    fn backend_key_is_stable_for_reordered_config() {
        let left = BackendInstanceKey::from_request(&request(
            "s1",
            json!({"api_url": "http://conch", "image_name": "img"}),
        ))
        .expect("key");
        let right = BackendInstanceKey::from_request(&request(
            "s1",
            json!({"image_name": "img", "api_url": "http://conch"}),
        ))
        .expect("key");

        assert_eq!(left, right);
    }

    #[test]
    fn backend_key_splits_by_session_and_config() {
        let base = BackendInstanceKey::from_request(&request(
            "s1",
            json!({"api_url": "http://conch", "image_name": "img"}),
        ))
        .expect("key");
        let other_session = BackendInstanceKey::from_request(&request(
            "s2",
            json!({"api_url": "http://conch", "image_name": "img"}),
        ))
        .expect("key");
        let other_config = BackendInstanceKey::from_request(&request(
            "s1",
            json!({"api_url": "http://conch", "image_name": "other"}),
        ))
        .expect("key");

        assert_ne!(base, other_session);
        assert_ne!(base, other_config);
    }

    #[test]
    fn conch_backend_config_injects_workspace_root_from_request() {
        let config = operation_config_for_acquire(&request(
            "s1",
            json!({"api_url": "http://conch", "image_name": "img"}),
        ))
        .expect("config");

        assert_eq!(config.options["workspace_root"], "/workspace");
    }

    #[test]
    fn conch_backend_config_overrides_user_workspace_root() {
        let config = operation_config_for_acquire(&request(
            "s1",
            json!({
                "api_url": "http://conch",
                "image_name": "img",
                "workspace_root": "/wrong"
            }),
        ))
        .expect("config");

        assert_eq!(config.options["workspace_root"], "/workspace");
    }
}
