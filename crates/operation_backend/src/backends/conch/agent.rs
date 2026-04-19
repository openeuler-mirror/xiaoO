use crate::backends::conch::backend::ConchBackendState;
use crate::backends::conch::backend::ConchExecOutput;
use crate::backends::conch::backend::ConchSandboxHandle;
use crate::backends::conch::backend::ConchStartProcess;
use crate::backends::conch::backend::ConchUploadFile;
use agent_contracts::backend::OperationError;
use tonic::transport::{Channel, Endpoint};

pub(crate) mod proto {
    tonic::include_proto!("pb");
}

pub(crate) async fn health_check(handle: &ConchSandboxHandle) -> Result<(), OperationError> {
    let mut client = client(handle).await?;
    let response = client
        .health_check(proto::Empty {})
        .await
        .map_err(grpc_error)?
        .into_inner();
    if response.message == "OK" {
        return Ok(());
    }
    Err(OperationError::Transport {
        message: format!("unexpected conch health response: {}", response.message),
    })
}

pub(crate) async fn start_process(
    state: &ConchBackendState,
    request: ConchStartProcess,
) -> Result<ConchExecOutput, OperationError> {
    state.ensure_active()?;
    let mut client = client(&state.sandbox).await?;
    let response = client
        .start_process(proto::StartProcessRequest {
            cmd: request.cmd,
            args: request.args,
            env: request.env,
            cwd: request.cwd.unwrap_or_default(),
            content: request.content.unwrap_or_default(),
        })
        .await
        .map_err(grpc_error)?
        .into_inner();

    if !response.error.is_empty() {
        return Err(OperationError::ExecutionFailed {
            message: response.error,
        });
    }

    Ok(ConchExecOutput {
        stdout: response.stdout.into_bytes(),
        stderr: response.stderr.into_bytes(),
        exit_code: Some(response.exit_code),
        timed_out: false,
    })
}

pub(crate) async fn post_files(
    state: &ConchBackendState,
    files: Vec<ConchUploadFile>,
) -> Result<i32, OperationError> {
    state.ensure_active()?;
    let mut client = client(&state.sandbox).await?;
    let response = client
        .post_files(proto::PostFilesRequest {
            files: files
                .into_iter()
                .map(|file| proto::File {
                    filepath: file.filepath,
                    content: file.content,
                })
                .collect(),
        })
        .await
        .map_err(grpc_error)?
        .into_inner();

    if !response.error.is_empty() {
        return Err(OperationError::Transport {
            message: response.error,
        });
    }

    Ok(response.uploaded_count)
}

pub(crate) async fn get_file(
    state: &ConchBackendState,
    filepath: &str,
) -> Result<Vec<u8>, OperationError> {
    state.ensure_active()?;
    let mut client = client(&state.sandbox).await?;
    let response = client
        .get_file(proto::GetFileRequest {
            filepath: filepath.to_string(),
        })
        .await
        .map_err(grpc_error)?
        .into_inner();

    if !response.error.is_empty() {
        if response.error.contains("file not found") {
            return Err(OperationError::NotFound {
                path: filepath.to_string(),
            });
        }
        return Err(OperationError::Transport {
            message: response.error,
        });
    }

    Ok(response.content)
}

async fn client(
    handle: &ConchSandboxHandle,
) -> Result<proto::agent_service_client::AgentServiceClient<Channel>, OperationError> {
    let endpoint = Endpoint::from_shared(format!("http://{}:{}", handle.ip, handle.agent_port))
        .map_err(|error| OperationError::Transport {
            message: format!("invalid conch agent endpoint: {error}"),
        })?;
    let channel = endpoint.connect().await.map_err(|error| OperationError::Transport {
        message: format!("failed to connect conch agent: {error}"),
    })?;
    Ok(proto::agent_service_client::AgentServiceClient::new(channel))
}

fn grpc_error(error: tonic::Status) -> OperationError {
    OperationError::Transport {
        message: format!("conch agent rpc failed: {error}"),
    }
}
