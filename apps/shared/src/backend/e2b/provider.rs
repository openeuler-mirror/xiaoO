use agent_contracts::backend::{
    BackendEndpoint, BackendId, BackendInstance, BackendInstanceId, BackendLifecycleState,
    BackendPath, BackendProviderKind, BackendResourceAllocation, BackendRuntimeCapabilities,
    OperationBackend, OperationError,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::backend::{
    join_url, normalize_backend_path, E2bBackendState, E2bLifecycle, E2bOperationBackend,
    DEFAULT_API_BASE, DEFAULT_ENVD_PORT, DEFAULT_HOME_DIR, DEFAULT_SHELL, DEFAULT_TEMPLATE_ID,
    DEFAULT_TEMP_ROOT, DEFAULT_TIMEOUT_SECS, DEFAULT_WORKSPACE_ROOT, E2B_PROVIDER_KIND,
};
use super::exec::E2bExec;
use crate::backend::BackendError;

pub(crate) struct E2bCreateBackendInput {
    pub(crate) backend_id: BackendId,
    pub(crate) session_id_for_instance: String,
    pub(crate) workspace_root_text: String,
    pub(crate) provider_options: Value,
    pub(crate) resource_limits: agent_contracts::backend::BackendResourceLimits,
    pub(crate) metadata: Value,
}

pub(crate) struct E2bCreatedBackend {
    pub(crate) instance: BackendInstance,
    pub(crate) backend: Arc<dyn OperationBackend>,
}

pub(crate) struct E2bSnapshotInput {
    pub(crate) provider_options: Value,
    pub(crate) sandbox_id: String,
    pub(crate) name: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct E2bSnapshotResult {
    pub(crate) snapshot_id: String,
    pub(crate) names: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct E2bProviderOptions {
    api_key: Option<String>,
    api_key_env: Option<String>,
    #[serde(alias = "apiBase")]
    api_base: Option<String>,
    #[serde(alias = "templateID", alias = "template")]
    template_id: Option<String>,
    #[serde(alias = "timeout")]
    timeout_secs: Option<u64>,
    secure: Option<bool>,
    #[serde(alias = "allowInternetAccess")]
    allow_internet_access: Option<bool>,
    #[serde(alias = "autoPause")]
    auto_pause: Option<bool>,
    #[serde(alias = "autoResume")]
    auto_resume: Option<bool>,
    #[serde(alias = "envdPort")]
    envd_port: Option<u16>,
    #[serde(alias = "envdScheme")]
    envd_scheme: Option<String>,
    #[serde(alias = "workspaceRoot", alias = "remoteWorkspaceRoot")]
    workspace_root: Option<String>,
    #[serde(alias = "homeDir")]
    home_dir: Option<String>,
    #[serde(alias = "tempRoot")]
    temp_root: Option<String>,
    #[serde(alias = "defaultShell")]
    default_shell: Option<String>,
    username: Option<String>,
    metadata: Option<BTreeMap<String, String>>,
    #[serde(alias = "envVars")]
    env_vars: Option<BTreeMap<String, String>>,
    network: Option<Value>,
    mcp: Option<Value>,
    #[serde(alias = "volumeMounts")]
    volume_mounts: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateSandboxResponse {
    #[serde(rename = "sandboxID")]
    sandbox_id: String,
    #[serde(rename = "templateID")]
    template_id: String,
    #[serde(rename = "envdAccessToken")]
    envd_access_token: Option<String>,
    #[serde(rename = "trafficAccessToken")]
    traffic_access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateSnapshotResponse {
    #[serde(rename = "snapshotID")]
    snapshot_id: String,
    #[serde(default)]
    names: Vec<String>,
}

pub(crate) async fn create_backend(
    input: E2bCreateBackendInput,
) -> Result<E2bCreatedBackend, BackendError> {
    let options = parse_options(&input.provider_options)?;
    let api_key = resolve_api_key(&options)?;
    let http = reqwest::Client::new();
    let api_base = options
        .api_base
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .to_string();
    let template_id = options
        .template_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_TEMPLATE_ID)
        .to_string();
    let workspace_root = backend_path(
        options
            .workspace_root
            .as_deref()
            .unwrap_or(DEFAULT_WORKSPACE_ROOT),
    )?;
    let home_dir = options
        .home_dir
        .as_deref()
        .map(backend_path)
        .transpose()?
        .or_else(|| Some(BackendPath(DEFAULT_HOME_DIR.to_string())));
    let temp_root = backend_path(options.temp_root.as_deref().unwrap_or(DEFAULT_TEMP_ROOT))?;
    let envd_port = options.envd_port.unwrap_or(DEFAULT_ENVD_PORT);
    let envd_scheme = options
        .envd_scheme
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https".to_string());

    let created = create_e2b_sandbox(
        &http,
        api_base.as_str(),
        api_key.as_str(),
        &template_id,
        &options,
        &input,
    )
    .await?;

    let now = current_time_ms();
    let backend_id = input.backend_id;
    let endpoint = provider_handle(&created, envd_port, envd_scheme.as_str());
    let instance = BackendInstance {
        backend_id: backend_id.clone(),
        provider: BackendProviderKind(E2B_PROVIDER_KIND.to_string()),
        instance_id: BackendInstanceId(created.sandbox_id.clone()),
        session_id: input.session_id_for_instance,
        state: BackendLifecycleState::Active,
        workspace_root: workspace_root.clone(),
        endpoint: Some(endpoint),
        snapshot: None,
        capabilities: BackendRuntimeCapabilities {
            supports_exec: true,
            supports_file_read: true,
            supports_file_write: true,
            supports_search: true,
            supports_export_file: true,
            supports_lsp: false,
            supports_pause: false,
            supports_snapshot: true,
            supports_delete: true,
        },
        resources: BackendResourceAllocation {
            vcpu_count: input.resource_limits.vcpu_count,
            memory_mb: input.resource_limits.memory_mb,
            disk_mb: input.resource_limits.disk_mb,
        },
        metadata: metadata_for_instance(
            input.metadata,
            &input.provider_options,
            &created,
            &options,
        ),
        created_at_ms: now,
        updated_at_ms: now,
    };

    let state = Arc::new(E2bBackendState {
        backend_id: backend_id.0,
        api_base,
        api_key,
        sandbox_id: created.sandbox_id,
        envd_access_token: created.envd_access_token,
        envd_port,
        envd_scheme,
        workspace_root,
        home_dir,
        temp_root,
        default_shell: Some(
            options
                .default_shell
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| DEFAULT_SHELL.to_string()),
        ),
        username: options.username.filter(|value| !value.trim().is_empty()),
        http,
        lifecycle: Mutex::new(E2bLifecycle::Active),
    });
    let backend = Arc::new(E2bOperationBackend::new(Arc::clone(&state)));

    if let Err(error) = ensure_remote_roots(&state).await {
        let _ = state.delete_sandbox().await;
        return Err(BackendError::BuildFailed {
            message: format!("e2b sandbox created but workspace initialization failed: {error}"),
        });
    }

    Ok(E2bCreatedBackend { instance, backend })
}

pub(crate) async fn create_snapshot(
    input: E2bSnapshotInput,
) -> Result<E2bSnapshotResult, BackendError> {
    let options = parse_options(&input.provider_options)?;
    let api_key = resolve_api_key(&options)?;
    let api_base = options
        .api_base
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .to_string();
    let http = reqwest::Client::new();

    let mut body = Map::new();
    if let Some(name) = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        body.insert("name".to_string(), Value::String(name.to_string()));
    }

    let response = http
        .post(join_url(
            api_base.as_str(),
            format!("/sandboxes/{}/snapshots", input.sandbox_id).as_str(),
        ))
        .header("X-API-Key", api_key)
        .json(&Value::Object(body))
        .send()
        .await
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to create e2b snapshot: {error}"),
        })?;

    if response.status() != reqwest::StatusCode::CREATED {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let message = super::backend::parse_error_message(text.as_str()).unwrap_or(text);
        return Err(BackendError::BuildFailed {
            message: format!("e2b create snapshot failed with HTTP {status}: {message}"),
        });
    }

    let parsed = response
        .json::<CreateSnapshotResponse>()
        .await
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to decode e2b create snapshot response: {error}"),
        })?;

    Ok(E2bSnapshotResult {
        snapshot_id: parsed.snapshot_id,
        names: parsed.names,
    })
}

fn parse_options(value: &Value) -> Result<E2bProviderOptions, BackendError> {
    let value = if value.is_null() {
        Value::Object(Map::new())
    } else {
        value.clone()
    };
    serde_json::from_value(value).map_err(|error| BackendError::InvalidRequest {
        message: format!("invalid e2b backend options: {error}"),
    })
}

fn resolve_api_key(options: &E2bProviderOptions) -> Result<String, BackendError> {
    if let Some(api_key) = options
        .api_key
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        return Ok(api_key.to_string());
    }

    let env_name = options
        .api_key_env
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("E2B_API_KEY");
    std::env::var(env_name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| BackendError::InvalidRequest {
            message: format!("e2b backend requires api_key or non-empty env var {env_name}"),
        })
}

fn backend_path(value: &str) -> Result<BackendPath, BackendError> {
    normalize_backend_path(std::path::Path::new(value)).map_err(|error| {
        BackendError::InvalidRequest {
            message: error.to_string(),
        }
    })
}

async fn create_e2b_sandbox(
    http: &reqwest::Client,
    api_base: &str,
    api_key: &str,
    template_id: &str,
    options: &E2bProviderOptions,
    input: &E2bCreateBackendInput,
) -> Result<CreateSandboxResponse, BackendError> {
    let timeout_secs = options
        .timeout_secs
        .or_else(|| input.resource_limits.timeout_ms.map(|ms| ms / 1000))
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    let mut body = Map::new();
    body.insert(
        "templateID".to_string(),
        Value::String(template_id.to_string()),
    );
    body.insert("timeout".to_string(), json!(timeout_secs));
    body.insert("secure".to_string(), json!(options.secure.unwrap_or(true)));
    if let Some(value) = options.allow_internet_access {
        body.insert("allow_internet_access".to_string(), json!(value));
    }
    if let Some(value) = options.auto_pause {
        body.insert("autoPause".to_string(), json!(value));
    }
    if let Some(value) = options.auto_resume {
        body.insert("autoResume".to_string(), json!({ "enabled": value }));
    }
    let metadata = platform_metadata(input, options);
    if !metadata.is_empty() {
        body.insert(
            "metadata".to_string(),
            serde_json::to_value(metadata).unwrap(),
        );
    }
    if let Some(env_vars) = options
        .env_vars
        .as_ref()
        .filter(|values| !values.is_empty())
    {
        body.insert(
            "envVars".to_string(),
            serde_json::to_value(env_vars).unwrap(),
        );
    }
    if let Some(network) = options.network.clone() {
        body.insert("network".to_string(), network);
    }
    if let Some(mcp) = options.mcp.clone() {
        body.insert("mcp".to_string(), mcp);
    }
    if let Some(volume_mounts) = options.volume_mounts.clone() {
        body.insert("volumeMounts".to_string(), volume_mounts);
    }

    let response = http
        .post(join_url(api_base, "/sandboxes"))
        .header("X-API-Key", api_key)
        .json(&Value::Object(body))
        .send()
        .await
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to create e2b sandbox: {error}"),
        })?;

    if response.status() != reqwest::StatusCode::CREATED {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let message = super::backend::parse_error_message(text.as_str()).unwrap_or(text);
        return Err(BackendError::BuildFailed {
            message: format!("e2b create sandbox failed with HTTP {status}: {message}"),
        });
    }

    response
        .json::<CreateSandboxResponse>()
        .await
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to decode e2b create sandbox response: {error}"),
        })
}

fn platform_metadata(
    input: &E2bCreateBackendInput,
    options: &E2bProviderOptions,
) -> BTreeMap<String, String> {
    let mut metadata = BTreeMap::new();
    if let Some(values) = &options.metadata {
        metadata.extend(values.clone());
    }
    if let Some(object) = input.metadata.as_object() {
        for (key, value) in object {
            if let Some(value) = value.as_str() {
                metadata.insert(key.clone(), value.to_string());
            }
        }
    }
    metadata.insert("xiaoo_backend_id".to_string(), input.backend_id.0.clone());
    metadata.insert(
        "xiaoo_session_id".to_string(),
        input.session_id_for_instance.clone(),
    );
    metadata.insert(
        "xiaoo_host_workspace".to_string(),
        input.workspace_root_text.clone(),
    );
    metadata
}

fn metadata_for_instance(
    metadata: Value,
    provider_options: &Value,
    sandbox: &CreateSandboxResponse,
    options: &E2bProviderOptions,
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
    object.insert("provider".to_string(), Value::String("e2b".to_string()));
    object.insert(
        "sandbox_id".to_string(),
        Value::String(sandbox.sandbox_id.clone()),
    );
    object.insert(
        "template_id".to_string(),
        Value::String(sandbox.template_id.clone()),
    );
    if sandbox.traffic_access_token.is_some() {
        object.insert(
            "traffic_access_token_present".to_string(),
            Value::Bool(true),
        );
    }
    object.insert(
        "provider_options".to_string(),
        redacted_provider_options(provider_options, options),
    );
    Value::Object(object)
}

fn redacted_provider_options(provider_options: &Value, options: &E2bProviderOptions) -> Value {
    let mut object = provider_options.as_object().cloned().unwrap_or_default();
    object.remove("api_key");
    object.remove("env_vars");
    object.remove("envVars");
    if options.api_key.is_some() {
        object.insert("api_key_configured".to_string(), Value::Bool(true));
    }
    Value::Object(object)
}

fn provider_handle(
    sandbox: &CreateSandboxResponse,
    envd_port: u16,
    envd_scheme: &str,
) -> BackendEndpoint {
    BackendEndpoint::ProviderHandle {
        value: json!({
            "provider": "e2b",
            "sandbox_id": sandbox.sandbox_id.clone(),
            "envd_host": format!("{}-{}.e2b.app", envd_port, sandbox.sandbox_id),
            "envd_port": envd_port,
            "envd_scheme": envd_scheme,
        }),
    }
}

async fn ensure_remote_roots(state: &Arc<E2bBackendState>) -> Result<(), OperationError> {
    let exec = E2bExec::new(Arc::clone(state));
    let script = format!(
        "mkdir -p {} {}",
        super::backend::shell_quote(state.workspace_root.0.as_str()),
        super::backend::shell_quote(state.temp_root.0.as_str())
    );
    let output = exec.run_shell_script(script.as_str(), None).await?;
    if output.exit_code == Some(0) {
        return Ok(());
    }
    Err(OperationError::ExecutionFailed {
        message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
    })
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

    #[test]
    fn redacts_direct_api_key_from_metadata() {
        let options = parse_options(&json!({
            "api_key": "secret",
            "template_id": "base",
            "envVars": {"TOKEN": "secret"}
        }))
        .expect("options");

        let redacted = redacted_provider_options(
            &json!({
                "api_key": "secret",
                "template_id": "base",
                "envVars": {"TOKEN": "secret"}
            }),
            &options,
        );

        let object = redacted.as_object().expect("object");
        assert!(!object.contains_key("api_key"));
        assert!(!object.contains_key("envVars"));
        assert_eq!(object.get("api_key_configured"), Some(&Value::Bool(true)));
    }

    #[test]
    fn default_template_is_base() {
        let options = parse_options(&json!({})).expect("options");
        assert_eq!(
            options
                .template_id
                .as_deref()
                .unwrap_or(DEFAULT_TEMPLATE_ID),
            "base"
        );
    }
}
