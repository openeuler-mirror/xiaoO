use crate::gateway::backend::conch::agent;
use crate::gateway::backend::conch::backend::{
    normalize_backend_path, ConchBackendState, ConchControlPlane, ConchControlTransport,
    ConchLifecycle, ConchOperationBackend, ConchSandboxHandle,
};
use crate::gateway::backend::conch::control::{self, ConchCreateOptions};
use agent_contracts::backend::{
    BackendPath, OperationBackend, OperationBackendBuildError, OperationBackendConfig,
};
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConchBackendOptions {
    unix_socket: Option<String>,
    api_url: Option<String>,
    namespace: Option<String>,
    image_name: Option<String>,
    snapshot_id: Option<String>,
    use_snapshot: Option<bool>,
    vmm_name: Option<String>,
    vcpu_num: Option<i64>,
    ram_mb: Option<i64>,
    workspace_root: String,
    home_dir: Option<String>,
    temp_root: Option<String>,
    default_shell: Option<String>,
    sandbox_id_prefix: Option<String>,
    sandbox_id: Option<String>,
}

struct ValidatedConchConfig {
    backend_id: String,
    workspace_root: BackendPath,
    home_dir: Option<BackendPath>,
    temp_root: BackendPath,
    default_shell: Option<String>,
    control_plane: ConchControlPlane,
    create_options: ConchCreateOptions,
}

pub async fn build_backend(
    config: &OperationBackendConfig,
) -> Result<Arc<dyn OperationBackend>, OperationBackendBuildError> {
    let validated = validate_conch_config(config)?;
    let sandbox = control::create_sandbox(
        &validated.control_plane.transport,
        &validated.create_options,
    )
    .await?;
    if let Err(error) = agent::health_check(&sandbox).await {
        let cleanup_state = build_state(&validated, sandbox.clone());
        let cleanup_result = control::delete_sandbox(&cleanup_state).await;
        let message = match cleanup_result {
            Ok(()) => format!("conch sandbox health check failed: {error}"),
            Err(cleanup_error) => format!(
                "conch sandbox health check failed: {error}; cleanup delete also failed: {cleanup_error}"
            ),
        };
        return Err(OperationBackendBuildError::BuildFailed { message });
    }

    let state = Arc::new(build_state(&validated, sandbox));

    Ok(Arc::new(ConchOperationBackend::new(state)))
}

fn validate_conch_config(
    config: &OperationBackendConfig,
) -> Result<ValidatedConchConfig, OperationBackendBuildError> {
    let options: ConchBackendOptions =
        serde_json::from_value(config.options.clone()).map_err(|error| {
            OperationBackendBuildError::InvalidConfig {
                message: format!("invalid conch backend options: {error}"),
            }
        })?;

    let transport = match (options.unix_socket.as_deref(), options.api_url.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(OperationBackendBuildError::InvalidConfig {
                message: "conch backend config cannot specify both unix_socket and api_url"
                    .to_string(),
            });
        }
        (Some(path), None) => ConchControlTransport::UnixSocket(PathBuf::from(path)),
        (None, Some(url)) => ConchControlTransport::ApiUrl(url.to_string()),
        (None, None) => {
            return Err(OperationBackendBuildError::InvalidConfig {
                message: "conch backend requires unix_socket or api_url".to_string(),
            });
        }
    };

    if options.image_name.is_none()
        && options
            .snapshot_id
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        return Err(OperationBackendBuildError::InvalidConfig {
            message: "conch backend requires image_name when snapshot_id is absent".to_string(),
        });
    }

    let sandbox_id = options.sandbox_id.unwrap_or_else(|| {
        let prefix = options
            .sandbox_id_prefix
            .as_deref()
            .unwrap_or("xiaoo-conch")
            .to_string();
        format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
    });

    let workspace_root = normalize_path_string(options.workspace_root.as_str())?;
    let home_dir = options
        .home_dir
        .as_deref()
        .map(normalize_path_string)
        .transpose()?;
    let temp_root = options
        .temp_root
        .as_deref()
        .map(normalize_path_string)
        .transpose()?
        .unwrap_or_else(|| BackendPath("/tmp".to_string()));
    let namespace = options.namespace.unwrap_or_default();

    Ok(ValidatedConchConfig {
        backend_id: format!("conch:{sandbox_id}"),
        workspace_root,
        home_dir,
        temp_root,
        default_shell: options.default_shell,
        control_plane: ConchControlPlane {
            transport,
            namespace: namespace.clone(),
        },
        create_options: ConchCreateOptions {
            namespace,
            sandbox_id,
            image_name: options.image_name.unwrap_or_default(),
            snapshot_id: options.snapshot_id,
            use_snapshot: options.use_snapshot.unwrap_or(false),
            vmm_name: options
                .vmm_name
                .unwrap_or_else(|| "cloud-hypervisor".to_string()),
            vcpu_num: options.vcpu_num.unwrap_or(1),
            ram_mb: options.ram_mb.unwrap_or(1024),
        },
    })
}

fn build_state(validated: &ValidatedConchConfig, sandbox: ConchSandboxHandle) -> ConchBackendState {
    ConchBackendState {
        backend_id: validated.backend_id.clone(),
        workspace_root: validated.workspace_root.clone(),
        home_dir: validated.home_dir.clone(),
        temp_root: validated.temp_root.clone(),
        default_shell: validated.default_shell.clone(),
        control_plane: validated.control_plane.clone(),
        sandbox,
        lifecycle: Mutex::new(ConchLifecycle::Active),
    }
}

fn normalize_path_string(value: &str) -> Result<BackendPath, OperationBackendBuildError> {
    normalize_backend_path(std::path::Path::new(value)).map_err(|error| {
        OperationBackendBuildError::InvalidConfig {
            message: error.to_string(),
        }
    })
}
