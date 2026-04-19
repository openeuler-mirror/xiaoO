use crate::backends::conch::backend::{
    ConchBackendState, ConchControlTransport, ConchSandboxHandle,
};
use agent_contracts::backend::{OperationBackendBuildError, OperationError};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct ConchCreateOptions {
    pub(crate) namespace: String,
    pub(crate) sandbox_id: String,
    pub(crate) image_name: String,
    pub(crate) snapshot_id: Option<String>,
    pub(crate) use_snapshot: bool,
    pub(crate) vmm_name: String,
    pub(crate) vcpu_num: i64,
    pub(crate) ram_mb: i64,
}

#[derive(Debug, Serialize)]
struct SandboxCreateRequest<'a> {
    namespace: &'a str,
    snapshot_id: &'a str,
    image_name: &'a str,
    use_snapshot: bool,
    vmm_name: &'a str,
    sandbox_id: &'a str,
    vcpu_num: i64,
    ram_mb: i64,
}

#[derive(Debug, Deserialize)]
struct SandboxCreateResponse {
    status: String,
    ip: String,
}

#[derive(Debug, Serialize)]
struct SandboxDeleteRequest<'a> {
    namespace: &'a str,
    sandbox_id: &'a str,
}

#[derive(Debug, Deserialize)]
struct SandboxDeleteResponse {
    status: String,
}

pub(crate) async fn create_sandbox(
    transport: &ConchControlTransport,
    options: &ConchCreateOptions,
) -> Result<ConchSandboxHandle, OperationBackendBuildError> {
    let client = build_client(transport)?;
    let url = control_plane_url(transport, "/api/sandbox/create");
    let snapshot_id = options.snapshot_id.as_deref().unwrap_or("");
    let response = client
        .post(url)
        .json(&SandboxCreateRequest {
            namespace: options.namespace.as_str(),
            snapshot_id,
            image_name: options.image_name.as_str(),
            use_snapshot: options.use_snapshot,
            vmm_name: options.vmm_name.as_str(),
            sandbox_id: options.sandbox_id.as_str(),
            vcpu_num: options.vcpu_num,
            ram_mb: options.ram_mb,
        })
        .send()
        .await
        .map_err(|error| OperationBackendBuildError::BuildFailed {
            message: format!("failed to call conch create endpoint: {error}"),
        })?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| OperationBackendBuildError::BuildFailed {
            message: format!("failed to read conch create response: {error}"),
        })?;

    if !status.is_success() {
        return Err(OperationBackendBuildError::BuildFailed {
            message: format!("conch create failed with {status}: {body}"),
        });
    }

    let parsed: SandboxCreateResponse = serde_json::from_str(body.as_str()).map_err(|error| {
        OperationBackendBuildError::BuildFailed {
            message: format!("invalid conch create response: {error}"),
        }
    })?;
    if parsed.status != "ok" {
        return Err(OperationBackendBuildError::BuildFailed {
            message: format!("conch create returned non-ok status: {}", parsed.status),
        });
    }

    Ok(ConchSandboxHandle {
        sandbox_id: options.sandbox_id.clone(),
        ip: parsed.ip,
        agent_port: 4064,
    })
}

pub(crate) async fn delete_sandbox(state: &ConchBackendState) -> Result<(), OperationError> {
    let client = build_client(&state.control_plane.transport).map_err(|error| OperationError::Transport {
        message: error.to_string(),
    })?;
    let url = control_plane_url(&state.control_plane.transport, "/api/sandbox/delete");
    let response = client
        .post(url)
        .json(&SandboxDeleteRequest {
            namespace: state.control_plane.namespace.as_str(),
            sandbox_id: state.sandbox.sandbox_id.as_str(),
        })
        .send()
        .await
        .map_err(|error| OperationError::Transport {
            message: format!("failed to call conch delete endpoint: {error}"),
        })?;
    let status = response.status();
    let body = response.text().await.map_err(|error| OperationError::Transport {
        message: format!("failed to read conch delete response: {error}"),
    })?;
    if !status.is_success() {
        return Err(OperationError::Transport {
            message: format!("conch delete failed with {status}: {body}"),
        });
    }

    let parsed: SandboxDeleteResponse = serde_json::from_str(body.as_str()).map_err(|error| {
        OperationError::Transport {
            message: format!("invalid conch delete response: {error}"),
        }
    })?;
    if parsed.status != "ok" {
        return Err(OperationError::Transport {
            message: format!("conch delete returned non-ok status: {}", parsed.status),
        });
    }
    Ok(())
}

fn build_client(
    transport: &ConchControlTransport,
) -> Result<Client, OperationBackendBuildError> {
    let builder = Client::builder();
    match transport {
        ConchControlTransport::ApiUrl(_) => builder.build().map_err(|error| {
            OperationBackendBuildError::BuildFailed {
                message: format!("failed to build conch http client: {error}"),
            }
        }),
        ConchControlTransport::UnixSocket(path) => unix_socket_client(builder, path),
    }
}

#[cfg(unix)]
fn unix_socket_client(
    builder: reqwest::ClientBuilder,
    path: &PathBuf,
) -> Result<Client, OperationBackendBuildError> {
    builder
        .unix_socket(path.clone())
        .build()
        .map_err(|error| OperationBackendBuildError::BuildFailed {
            message: format!("failed to build conch unix-socket client: {error}"),
        })
}

#[cfg(not(unix))]
fn unix_socket_client(
    _builder: reqwest::ClientBuilder,
    _path: &PathBuf,
) -> Result<Client, OperationBackendBuildError> {
    Err(OperationBackendBuildError::BuildFailed {
        message: "unix socket control plane is only supported on unix hosts".to_string(),
    })
}

fn control_plane_url(transport: &ConchControlTransport, path: &str) -> String {
    match transport {
        ConchControlTransport::ApiUrl(base_url) => format!("{}{}", base_url.trim_end_matches('/'), path),
        ConchControlTransport::UnixSocket(_) => format!("http://localhost{path}"),
    }
}
