use super::factory::local_backend_with_isolation;
use agent_contracts::backend::{
    BackendControlError, BackendCreateRequest, BackendDeleteOutcome, BackendDeleteRequest,
    BackendEndpoint, BackendId, BackendInspectRequest, BackendInstance, BackendInstanceId,
    BackendInstanceStatus, BackendLifecycle, BackendLifecycleOperation, BackendLifecycleState,
    BackendLifecycleStateMachine, BackendLoadRequest, BackendPauseRequest, BackendProvider,
    BackendProviderKind, BackendResourceAllocation, BackendRuntimeCapabilities, BackendSnapshot,
    OperationBackend,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Arc;

const LOCAL_PROVIDER_KIND: &str = "local";
const LOCAL_METADATA_PROVIDER_OPTIONS: &str = "provider_options";
const LOCAL_METADATA_WORKSPACE_ROOT: &str = "workspace_root";

#[derive(Debug, Clone, Default)]
pub struct LocalBackendProvider;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LocalProviderOptions {
    home_dir: Option<String>,
    temp_root: Option<String>,
    default_shell: Option<String>,
    isolation: Option<Value>,
}

impl LocalBackendProvider {
    pub fn new() -> Self {
        Self
    }

    fn provider_kind() -> BackendProviderKind {
        BackendProviderKind(LOCAL_PROVIDER_KIND.to_string())
    }

    fn capabilities() -> BackendRuntimeCapabilities {
        BackendRuntimeCapabilities {
            supports_exec: true,
            supports_file_read: true,
            supports_file_write: true,
            supports_search: true,
            supports_export_file: true,
            supports_lsp: true,
            supports_pause: false,
            supports_snapshot: false,
            supports_delete: true,
        }
    }

    fn backend_id(session_id: &str) -> BackendId {
        BackendId(format!("{LOCAL_PROVIDER_KIND}:{session_id}"))
    }

    fn instance_id(session_id: &str) -> BackendInstanceId {
        BackendInstanceId(format!("{LOCAL_PROVIDER_KIND}:{session_id}"))
    }

    fn parse_options(value: &Value) -> Result<LocalProviderOptions, BackendControlError> {
        let value = if value.is_null() {
            Value::Object(Map::new())
        } else {
            value.clone()
        };
        serde_json::from_value(value).map_err(|error| BackendControlError::InvalidRequest {
            message: format!("invalid local backend provider options: {error}"),
        })
    }

    fn options_from_metadata(
        metadata: &Value,
    ) -> Result<LocalProviderOptions, BackendControlError> {
        let options = metadata
            .get(LOCAL_METADATA_PROVIDER_OPTIONS)
            .cloned()
            .unwrap_or_else(|| Value::Object(Map::new()));
        Self::parse_options(&options)
    }

    fn metadata_with_provider_options(
        metadata: Value,
        provider_options: Value,
        workspace_root: &agent_contracts::backend::BackendPath,
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
            LOCAL_METADATA_PROVIDER_OPTIONS.to_string(),
            provider_options,
        );
        object.insert(
            LOCAL_METADATA_WORKSPACE_ROOT.to_string(),
            Value::String(workspace_root.0.clone()),
        );
        Value::Object(object)
    }

    fn build_operation_backend(
        workspace_root: &agent_contracts::backend::BackendPath,
        options: LocalProviderOptions,
    ) -> Result<Arc<dyn OperationBackend>, BackendControlError> {
        local_backend_with_isolation(
            PathBuf::from(workspace_root.0.as_str()),
            options.home_dir.map(PathBuf::from),
            options.temp_root.map(PathBuf::from),
            options.default_shell,
            options.isolation,
        )
        .map_err(|error| BackendControlError::ProviderError {
            provider: Self::provider_kind(),
            message: error.to_string(),
        })
    }

    fn active_instance(
        backend_id: BackendId,
        session_id: String,
        workspace_root: agent_contracts::backend::BackendPath,
        provider_options: Value,
        metadata: Value,
        resources: BackendResourceAllocation,
        created_at_ms: u64,
        updated_at_ms: u64,
    ) -> BackendInstance {
        BackendInstance {
            backend_id,
            provider: Self::provider_kind(),
            instance_id: Self::instance_id(session_id.as_str()),
            session_id,
            state: BackendLifecycleState::Active,
            workspace_root: workspace_root.clone(),
            endpoint: Some(BackendEndpoint::Local),
            snapshot: None,
            capabilities: Self::capabilities(),
            resources,
            metadata: Self::metadata_with_provider_options(
                metadata,
                provider_options,
                &workspace_root,
            ),
            created_at_ms,
            updated_at_ms,
        }
    }
}

pub fn local_backend_provider() -> LocalBackendProvider {
    LocalBackendProvider::new()
}

#[async_trait]
impl BackendLifecycle for LocalBackendProvider {
    async fn create_sandbox(
        &self,
        request: BackendCreateRequest,
    ) -> Result<BackendInstance, BackendControlError> {
        let started =
            BackendLifecycleStateMachine::begin(None, BackendLifecycleOperation::CreateSandbox)?;
        let options = Self::parse_options(&request.provider_options)?;
        Self::build_operation_backend(&request.workspace_root, options)?;
        let state = BackendLifecycleStateMachine::complete_success(
            started,
            BackendLifecycleOperation::CreateSandbox,
        )?;
        debug_assert_eq!(state, BackendLifecycleState::Active);
        let now = current_time_ms();
        Ok(Self::active_instance(
            request
                .requested_backend_id
                .unwrap_or_else(|| Self::backend_id(request.session_id.as_str())),
            request.session_id,
            request.workspace_root,
            request.provider_options,
            request.metadata,
            BackendResourceAllocation {
                vcpu_count: request.resource_limits.vcpu_count,
                memory_mb: request.resource_limits.memory_mb,
                disk_mb: request.resource_limits.disk_mb,
            },
            now,
            now,
        ))
    }

    async fn load(
        &self,
        request: BackendLoadRequest,
    ) -> Result<BackendInstance, BackendControlError> {
        let started = BackendLifecycleStateMachine::begin(None, BackendLifecycleOperation::Load)?;
        let options = Self::parse_options(&request.provider_options)?;
        Self::build_operation_backend(&request.workspace_root, options)?;
        let state = BackendLifecycleStateMachine::complete_success(
            started,
            BackendLifecycleOperation::Load,
        )?;
        debug_assert_eq!(state, BackendLifecycleState::Active);
        let now = current_time_ms();
        Ok(Self::active_instance(
            Self::backend_id(request.session_id.as_str()),
            request.session_id,
            request.workspace_root,
            request.provider_options,
            request.metadata,
            BackendResourceAllocation {
                vcpu_count: request.resource_limits.vcpu_count,
                memory_mb: request.resource_limits.memory_mb,
                disk_mb: request.resource_limits.disk_mb,
            },
            now,
            now,
        ))
    }

    async fn pause(
        &self,
        _request: BackendPauseRequest,
    ) -> Result<BackendSnapshot, BackendControlError> {
        Err(BackendControlError::UnsupportedCapability {
            provider: Self::provider_kind(),
            capability: "pause".to_string(),
        })
    }

    async fn delete(
        &self,
        request: BackendDeleteRequest,
    ) -> Result<BackendDeleteOutcome, BackendControlError> {
        Ok(BackendDeleteOutcome {
            backend_id: request.backend_id,
            instance_id: request.instance_id,
            snapshot_id: request.snapshot_id,
            state: BackendLifecycleState::Deleted,
            already_deleted: false,
            metadata: request.metadata,
        })
    }

    async fn inspect(
        &self,
        request: BackendInspectRequest,
    ) -> Result<BackendInstanceStatus, BackendControlError> {
        if !request.has_target() {
            return Err(BackendControlError::InvalidRequest {
                message: "local backend inspect requires backend_id, instance_id, or snapshot_id"
                    .to_string(),
            });
        }

        let backend_id = request.backend_id.clone().unwrap_or_else(|| {
            request
                .instance_id
                .as_ref()
                .map(|id| BackendId(format!("{LOCAL_PROVIDER_KIND}:{}", id.0)))
                .unwrap_or_else(|| BackendId(LOCAL_PROVIDER_KIND.to_string()))
        });
        let (state, last_error) = inspect_workspace_state(&request.metadata);

        Ok(BackendInstanceStatus {
            backend_id,
            provider: Self::provider_kind(),
            instance_id: request.instance_id,
            state,
            endpoint: Some(BackendEndpoint::Local),
            snapshot: None,
            last_error,
            metadata: request.metadata,
            updated_at_ms: current_time_ms(),
        })
    }
}

#[async_trait]
impl BackendProvider for LocalBackendProvider {
    fn kind(&self) -> BackendProviderKind {
        Self::provider_kind()
    }

    fn lifecycle(&self) -> Arc<dyn BackendLifecycle> {
        Arc::new(Self::new())
    }

    async fn attach(
        &self,
        instance: BackendInstance,
    ) -> Result<Arc<dyn OperationBackend>, BackendControlError> {
        if !instance.state.operation_plane_usable() {
            return Err(BackendControlError::InvalidRequest {
                message: format!(
                    "local backend attach requires active instance, got {}",
                    instance.state
                ),
            });
        }
        let options = Self::options_from_metadata(&instance.metadata)?;
        Self::build_operation_backend(&instance.workspace_root, options)
    }
}

fn inspect_workspace_state(metadata: &Value) -> (BackendLifecycleState, Option<String>) {
    let Some(workspace_root) = metadata
        .get(LOCAL_METADATA_WORKSPACE_ROOT)
        .and_then(Value::as_str)
    else {
        return (BackendLifecycleState::Active, None);
    };

    match std::fs::metadata(workspace_root) {
        Ok(metadata) if metadata.is_dir() => (BackendLifecycleState::Active, None),
        Ok(_) => (
            BackendLifecycleState::Failed,
            Some(format!(
                "workspace_root is not a directory: {workspace_root}"
            )),
        ),
        Err(error) => (
            BackendLifecycleState::Failed,
            Some(format!(
                "failed to inspect workspace_root {workspace_root}: {error}"
            )),
        ),
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
    use agent_contracts::backend::{
        BackendLifecycleReason, BackendPath, BackendPauseMode, BackendResourceLimits,
        BackendSnapshotId,
    };

    #[test]
    fn local_delete_is_noop_success() {
        let provider = local_backend_provider();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");

        let outcome = runtime
            .block_on(provider.delete(BackendDeleteRequest {
                backend_id: BackendId("local:test".to_string()),
                instance_id: Some(BackendInstanceId("local:test".to_string())),
                snapshot_id: None,
                force: false,
                reason: BackendLifecycleReason::SessionClose,
                metadata: Value::Null,
            }))
            .expect("delete should succeed");

        assert_eq!(outcome.state, BackendLifecycleState::Deleted);
        assert!(!outcome.already_deleted);
    }

    #[test]
    fn local_pause_is_unsupported() {
        let provider = local_backend_provider();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");

        let result = runtime.block_on(provider.pause(BackendPauseRequest {
            backend_id: BackendId("local:test".to_string()),
            instance_id: BackendInstanceId("local:test".to_string()),
            mode: BackendPauseMode::BestEffort,
            reason: BackendLifecycleReason::SessionIdle,
            metadata: Value::Null,
        }));

        assert!(matches!(
            result,
            Err(BackendControlError::UnsupportedCapability { provider, capability })
                if provider.0 == LOCAL_PROVIDER_KIND && capability == "pause"
        ));
    }

    #[test]
    fn local_create_and_attach_builds_active_backend() {
        let provider = local_backend_provider();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let workspace_root = BackendPath(
            std::env::current_dir()
                .expect("cwd")
                .to_string_lossy()
                .to_string(),
        );

        let instance = runtime
            .block_on(provider.create_sandbox(BackendCreateRequest {
                requested_backend_id: None,
                session_id: "session".to_string(),
                conversation_id: None,
                workspace_root,
                provider_options: Value::Object(Map::new()),
                resource_limits: BackendResourceLimits::default(),
                metadata: Value::Null,
            }))
            .expect("create local instance");

        assert_eq!(instance.state, BackendLifecycleState::Active);
        assert_eq!(instance.provider.0, LOCAL_PROVIDER_KIND);

        let backend = runtime
            .block_on(provider.attach(instance))
            .expect("attach local backend");
        assert_eq!(backend.backend_id(), LOCAL_PROVIDER_KIND);
    }

    #[test]
    fn local_inspect_uses_workspace_metadata_when_present() {
        let provider = local_backend_provider();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let mut metadata = Map::new();
        metadata.insert(
            LOCAL_METADATA_WORKSPACE_ROOT.to_string(),
            Value::String("/path/that/should/not/exist/xiaoo-local-provider".to_string()),
        );

        let status = runtime
            .block_on(provider.inspect(BackendInspectRequest {
                backend_id: Some(BackendId("local:test".to_string())),
                instance_id: None,
                snapshot_id: Some(BackendSnapshotId("snapshot".to_string())),
                metadata: Value::Object(metadata),
            }))
            .expect("inspect local backend");

        assert_eq!(status.state, BackendLifecycleState::Failed);
        assert!(status.last_error.is_some());
    }
}
