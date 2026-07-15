use agent_contracts::backend::{
    BackendEndpoint, BackendId, BackendInstance, BackendInstanceId, BackendLifecycleState,
    BackendPath, BackendProviderKind, BackendResourceAllocation, BackendRuntimeCapabilities,
    OperationBackend, OperationError,
};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::backend::{
    join_url, normalize_backend_path, E2bBackendState, E2bLifecycle, E2bOperationBackend,
    DEFAULT_API_BASE, DEFAULT_ENVD_PORT, DEFAULT_HOME_DIR, DEFAULT_SHELL, DEFAULT_TEMPLATE_ID,
    DEFAULT_TEMP_ROOT, DEFAULT_TIMEOUT_SECS, DEFAULT_WORKSPACE_ROOT, E2B_PROVIDER_KIND,
};
use super::error::E2bFailure;
use super::exec::E2bExec;
use crate::backend::BackendError;

const E2B_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WORKSPACE_INIT_MAX_ATTEMPTS: usize = 6;
const WORKSPACE_INIT_BASE_DELAY_MS: u64 = 200;
const WORKSPACE_INIT_MAX_DELAY_MS: u64 = 2_000;

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

pub(crate) struct E2bDeleteSnapshotInput {
    pub(crate) provider_options: Value,
    pub(crate) snapshot_id: String,
}

pub(crate) struct E2bDeleteSandboxInput {
    pub(crate) provider_options: Value,
    pub(crate) sandbox_id: String,
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
    let http = new_e2b_http_client()?;
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
        if let Err(cleanup_error) = state.delete_sandbox().await {
            tracing::warn!(
                operation = "cleanup_failed_workspace_initialization",
                sandbox_id = %state.sandbox_id,
                error = %cleanup_error,
                "Failed to delete E2B sandbox after workspace initialization failure"
            );
        }
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
    let http = new_e2b_http_client()?;

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

pub(crate) async fn delete_snapshot(input: E2bDeleteSnapshotInput) -> Result<bool, BackendError> {
    let snapshot_id = input.snapshot_id.trim();
    if snapshot_id.is_empty() {
        return Err(BackendError::InvalidRequest {
            message: "e2b snapshot id cannot be empty".to_string(),
        });
    }

    let options = parse_options(&input.provider_options)?;
    let api_key = resolve_api_key(&options)?;
    let api_base = options
        .api_base
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .to_string();
    let http = new_e2b_http_client()?;

    let response = http
        .delete(join_url(
            api_base.as_str(),
            format!("/templates/{}", encode_path_segment(snapshot_id)).as_str(),
        ))
        .header("X-API-Key", api_key)
        .send()
        .await
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to delete e2b snapshot template: {error}"),
        })?;

    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(true);
    }
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let message = super::backend::parse_error_message(text.as_str()).unwrap_or(text);
    Err(BackendError::BuildFailed {
        message: format!("e2b delete snapshot template failed with HTTP {status}: {message}"),
    })
}

/// Delete an e2b sandbox by its id. This is used to reclaim sandboxes whose
/// owner process has died (the owner's in-memory `E2bBackendState` is gone,
/// so we can't call `backend.shutdown()` on it). Any process that has the
/// same api_key can call this directly against the e2b API.
pub(crate) async fn delete_sandbox_by_id(input: E2bDeleteSandboxInput) -> Result<(), BackendError> {
    let sandbox_id = input.sandbox_id.trim();
    if sandbox_id.is_empty() {
        return Err(BackendError::InvalidRequest {
            message: "e2b sandbox id cannot be empty".to_string(),
        });
    }

    let options = parse_options(&input.provider_options)?;
    let api_key = resolve_api_key(&options)?;
    let api_base = options
        .api_base
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(DEFAULT_API_BASE)
        .to_string();
    let http = new_e2b_http_client()?;

    let response = http
        .delete(join_url(
            api_base.as_str(),
            format!("/sandboxes/{}", encode_path_segment(sandbox_id)).as_str(),
        ))
        .header("X-API-Key", api_key)
        .send()
        .await
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to delete e2b sandbox: {error}"),
        })?;

    if response.status() == reqwest::StatusCode::NO_CONTENT
        || response.status() == reqwest::StatusCode::NOT_FOUND
    {
        return Ok(());
    }

    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    let message = super::backend::parse_error_message(text.as_str()).unwrap_or(text);
    Err(BackendError::BuildFailed {
        message: format!("e2b delete sandbox failed with HTTP {status}: {message}"),
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

/// Resolve the effective e2b api key from raw provider options, using the
/// same logic as `resolve_api_key` (direct `api_key` field, then the env var
/// named by `api_key_env`, defaulting to `E2B_API_KEY`). Returns `None` when
/// no key is configured — callers fall back to a stable default identifier.
pub(crate) fn resolve_api_key_from_options(options: &Value) -> Option<String> {
    let parsed = parse_options(options).ok()?;
    resolve_api_key(&parsed).ok()
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

    let started_at = Instant::now();
    let response = http
        .post(join_url(api_base, "/sandboxes"))
        .header("X-API-Key", api_key)
        .json(&Value::Object(body))
        .send()
        .await
        .map_err(|error| {
            let failure = E2bFailure::from_reqwest("failed to create e2b sandbox", &error);
            failure.log(
                "create_sandbox",
                None,
                None,
                started_at.elapsed().as_millis(),
            );
            BackendError::BuildFailed {
                message: failure.message,
            }
        })?;

    if response.status() != reqwest::StatusCode::CREATED {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let message = super::backend::parse_error_message(text.as_str()).unwrap_or(text);
        let failure = E2bFailure::from_status("e2b create sandbox", status, message.clone());
        failure.log(
            "create_sandbox",
            None,
            None,
            started_at.elapsed().as_millis(),
        );
        return Err(BackendError::BuildFailed {
            message: failure.message,
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
    let output = retry_workspace_initialization(
        state.sandbox_id.as_str(),
        || exec.run_shell_script_detailed(script.as_str(), None),
        tokio::time::sleep,
    )
    .await
    .map_err(super::exec::E2bExecFailure::into_operation_error)?;
    if output.exit_code == Some(0) {
        return Ok(());
    }
    Err(OperationError::ExecutionFailed {
        message: String::from_utf8_lossy(output.stderr.as_slice()).to_string(),
    })
}

async fn retry_workspace_initialization<Attempt, AttemptFuture, Sleep, SleepFuture>(
    sandbox_id: &str,
    mut operation: Attempt,
    mut sleep: Sleep,
) -> Result<super::exec::E2bExecOutput, super::exec::E2bExecFailure>
where
    Attempt: FnMut() -> AttemptFuture,
    AttemptFuture: Future<Output = Result<super::exec::E2bExecOutput, super::exec::E2bExecFailure>>,
    Sleep: FnMut(Duration) -> SleepFuture,
    SleepFuture: Future<Output = ()>,
{
    for attempt in 1..=WORKSPACE_INIT_MAX_ATTEMPTS {
        match operation().await {
            Ok(output) => return Ok(output),
            Err(error) if error.retryable() && attempt < WORKSPACE_INIT_MAX_ATTEMPTS => {
                let delay = workspace_init_backoff(attempt);
                tracing::warn!(
                    operation = "initialize_workspace",
                    sandbox_id,
                    attempt,
                    max_attempts = WORKSPACE_INIT_MAX_ATTEMPTS,
                    retry_delay_ms = delay.as_millis(),
                    error = %error.message(),
                    "E2B workspace initialization failed; retrying"
                );
                sleep(delay).await;
            }
            Err(error) => return Err(error),
        }
    }
    unreachable!("workspace initialization retry loop always returns")
}

fn workspace_init_backoff(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
    let base = WORKSPACE_INIT_BASE_DELAY_MS
        .saturating_mul(2u64.saturating_pow(exponent))
        .min(WORKSPACE_INIT_MAX_DELAY_MS);
    let spread = base / 5;
    let jitter = if spread == 0 {
        0
    } else {
        rand::random::<u64>() % (spread.saturating_mul(2).saturating_add(1))
    };
    Duration::from_millis(base.saturating_sub(spread).saturating_add(jitter))
}

fn new_e2b_http_client() -> Result<reqwest::Client, BackendError> {
    reqwest::Client::builder()
        .connect_timeout(E2B_CONNECT_TIMEOUT)
        .build()
        .map_err(|error| BackendError::BuildFailed {
            message: format!("failed to build e2b HTTP client: {error}"),
        })
}

fn current_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        match *byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(format!("%{byte:02X}").as_str()),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_contracts::backend::capability::exec::ExecRequest;
    use agent_contracts::backend::capability::filesystem::{WriteBytesRequest, WriteMode};
    use std::cell::Cell;
    use std::future::ready;

    #[tokio::test]
    #[ignore = "requires E2B_API_KEY and creates a real E2B sandbox"]
    async fn live_structured_grep_executes_command_with_args() {
        assert!(
            std::env::var_os("E2B_API_KEY").is_some(),
            "E2B_API_KEY must be set"
        );

        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let created = create_backend(E2bCreateBackendInput {
            backend_id: BackendId(format!("e2b-live-grep:{suffix}")),
            session_id_for_instance: format!("e2b-live-grep-session:{suffix}"),
            workspace_root_text: DEFAULT_WORKSPACE_ROOT.to_string(),
            provider_options: json!({
                "api_key_env": "E2B_API_KEY",
                "template_id": "base",
                "timeout_secs": 300,
                "default_shell": "/bin/sh"
            }),
            resource_limits: Default::default(),
            metadata: json!({"purpose": "structured grep live smoke"}),
        })
        .await
        .expect("create live E2B backend");
        let backend = created.backend;
        let smoke_path = BackendPath(format!("{DEFAULT_WORKSPACE_ROOT}/grep-smoke.py"));

        let smoke_result: Result<String, String> = async {
            backend
                .files()
                .write_bytes(WriteBytesRequest {
                    path: smoke_path,
                    content: b"watt = watts = W = Quantity(\"watt\")\n".to_vec(),
                    mode: WriteMode::Overwrite,
                })
                .await
                .map_err(|error| format!("write smoke fixture: {error}"))?;

            let output = backend
                .exec()
                .exec(ExecRequest {
                    command: "grep".to_string(),
                    args: vec![
                        "-P".to_string(),
                        "-n".to_string(),
                        "-e".to_string(),
                        r"W\s*=".to_string(),
                        "grep-smoke.py".to_string(),
                    ],
                    shell: None,
                    cwd: Some(BackendPath(DEFAULT_WORKSPACE_ROOT.to_string())),
                    timeout_ms: Some(30_000),
                    env: None,
                })
                .await
                .map_err(|error| format!("execute structured grep: {error}"))?;
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr);
            if output.exit_code != Some(0) {
                return Err(format!(
                    "grep exited with {:?}; stderr: {stderr}",
                    output.exit_code
                ));
            }
            if !stdout.contains("1:watt = watts = W = Quantity") {
                return Err(format!("unexpected grep stdout: {stdout:?}"));
            }
            Ok(stdout)
        }
        .await;

        let cleanup_result = backend.shutdown().await;
        cleanup_result.expect("delete live E2B sandbox");
        let stdout = smoke_result.expect("structured grep smoke");
        eprintln!("live E2B structured grep passed: {}", stdout.trim());
    }

    #[tokio::test]
    async fn workspace_initialization_retries_transient_failures() {
        let attempts = Cell::new(0usize);
        let sleeps = Cell::new(0usize);

        let output = retry_workspace_initialization(
            "sandbox-test",
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                ready(if attempt < 3 {
                    Err(super::super::exec::E2bExecFailure::retryable_for_test(
                        "temporary reset",
                    ))
                } else {
                    Ok(super::super::exec::E2bExecOutput {
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                        exit_code: Some(0),
                        timed_out: false,
                    })
                })
            },
            |_| {
                sleeps.set(sleeps.get() + 1);
                ready(())
            },
        )
        .await
        .expect("third attempt should succeed");

        assert_eq!(output.exit_code, Some(0));
        assert_eq!(attempts.get(), 3);
        assert_eq!(sleeps.get(), 2);
    }

    #[test]
    fn workspace_backoff_is_bounded_and_jittered() {
        for attempt in 1..=WORKSPACE_INIT_MAX_ATTEMPTS {
            let delay_ms = workspace_init_backoff(attempt).as_millis() as u64;
            let exponent = u32::try_from(attempt.saturating_sub(1)).unwrap_or(u32::MAX);
            let base = WORKSPACE_INIT_BASE_DELAY_MS
                .saturating_mul(2u64.saturating_pow(exponent))
                .min(WORKSPACE_INIT_MAX_DELAY_MS);
            assert!(delay_ms >= base - base / 5);
            assert!(delay_ms <= base + base / 5);
        }
    }

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

    #[test]
    fn encodes_template_id_as_single_path_segment() {
        assert_eq!(
            encode_path_segment("team/fork-test:default"),
            "team%2Ffork-test%3Adefault"
        );
    }
}
