use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    OperationError,
};
use async_trait::async_trait;
use base64::Engine;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use super::backend::{http_error, E2bBackendState, DEFAULT_SHELL};

pub(crate) struct E2bExec {
    state: Arc<E2bBackendState>,
}

impl E2bExec {
    pub(crate) fn new(state: Arc<E2bBackendState>) -> Self {
        Self { state }
    }

    pub(crate) fn state(&self) -> &Arc<E2bBackendState> {
        &self.state
    }

    pub(crate) async fn run_shell_script(
        &self,
        script: &str,
        cwd: Option<&str>,
    ) -> Result<E2bExecOutput, OperationError> {
        let shell = self
            .state
            .default_shell
            .as_deref()
            .unwrap_or(DEFAULT_SHELL)
            .to_string();
        start_process(
            &self.state,
            E2bStartProcess {
                cmd: shell,
                args: vec!["-c".to_string(), script.to_string()],
                env: HashMap::new(),
                cwd: cwd.map(str::to_string),
                connect_timeout_ms: None,
            },
        )
        .await
    }

    fn timeout_arg(timeout_ms: u64) -> String {
        let seconds = timeout_ms / 1000;
        let millis = timeout_ms % 1000;
        if millis == 0 {
            format!("{seconds}s")
        } else {
            format!("{seconds}.{millis:03}s")
        }
    }

    fn connect_timeout_ms(timeout_ms: Option<u64>) -> Option<u64> {
        timeout_ms.map(|value| value.saturating_add(10_000))
    }
}

pub(crate) struct E2bStartProcess {
    pub(crate) cmd: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: HashMap<String, String>,
    pub(crate) cwd: Option<String>,
    pub(crate) connect_timeout_ms: Option<u64>,
}

pub(crate) struct E2bExecOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
}

#[async_trait]
impl OperationExec for E2bExec {
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        self.state.ensure_active()?;
        if request.command.trim().is_empty() {
            return Err(OperationError::ExecutionFailed {
                message: "command cannot be empty".to_string(),
            });
        }

        let timeout_ms = request.timeout_ms;
        let env: HashMap<String, String> = request
            .env
            .as_ref()
            .map(|pairs| pairs.iter().cloned().collect())
            .unwrap_or_default();

        let (cmd, args) = if let Some(shell) = request
            .shell
            .clone()
            .or_else(|| self.state.default_shell.clone())
        {
            if !request.args.is_empty() {
                return Err(OperationError::Unsupported {
                    message: "shell execution does not support args".to_string(),
                });
            }
            if let Some(timeout_ms) = timeout_ms {
                (
                    "timeout".to_string(),
                    vec![
                        "--signal=TERM".to_string(),
                        Self::timeout_arg(timeout_ms),
                        shell,
                        "-c".to_string(),
                        request.command,
                    ],
                )
            } else {
                (shell, vec!["-c".to_string(), request.command])
            }
        } else if let Some(timeout_ms) = timeout_ms {
            let mut args = vec![
                "--signal=TERM".to_string(),
                Self::timeout_arg(timeout_ms),
                request.command,
            ];
            args.extend(request.args);
            ("timeout".to_string(), args)
        } else {
            (request.command, request.args)
        };

        let output = start_process(
            &self.state,
            E2bStartProcess {
                cmd,
                args,
                env,
                cwd: request.cwd.map(|path| path.0),
                connect_timeout_ms: Self::connect_timeout_ms(timeout_ms),
            },
        )
        .await?;
        let timed_out = timeout_ms.is_some() && output.exit_code == Some(124);

        Ok(ExecResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: output.timed_out || timed_out,
        })
    }
}

pub(crate) async fn start_process(
    state: &E2bBackendState,
    request: E2bStartProcess,
) -> Result<E2bExecOutput, OperationError> {
    state.ensure_active()?;
    let body = json!({
        "process": {
            "cmd": request.cmd,
            "args": request.args,
            "envs": request.env,
            "cwd": request.cwd,
        },
        "pty": null,
        "stdin": false,
    });

    let mut builder = state
        .envd_request(Method::POST, "/process.Process/Start")
        .header("Connect-Protocol-Version", "1")
        .header(CONTENT_TYPE, "application/connect+json")
        .header(ACCEPT, "application/connect+json")
        .body(connect_envelope(body.to_string().as_bytes()));
    if let Some(timeout_ms) = request.connect_timeout_ms {
        builder = builder.header("Connect-Timeout-Ms", timeout_ms.to_string());
    }

    let response = builder
        .send()
        .await
        .map_err(|error| OperationError::Transport {
            message: format!("failed to start e2b process: {error}"),
        })?;

    if !response.status().is_success() {
        return Err(http_error("start e2b process", response).await);
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|error| OperationError::Transport {
            message: format!("failed to read e2b process stream: {error}"),
        })?;
    parse_start_stream(bytes.as_ref())
}

fn parse_start_stream(bytes: &[u8]) -> Result<E2bExecOutput, OperationError> {
    if let Ok(response) = serde_json::from_slice::<StartResponse>(bytes) {
        return collect_start_response([response]);
    }

    let mut offset = 0usize;
    let mut responses = Vec::new();
    while offset < bytes.len() {
        if bytes.len().saturating_sub(offset) < 5 {
            return Err(OperationError::Transport {
                message: "malformed e2b process stream frame header".to_string(),
            });
        }

        let flags = bytes[offset];
        let len = u32::from_be_bytes([
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
            bytes[offset + 4],
        ]) as usize;
        offset += 5;
        if bytes.len().saturating_sub(offset) < len {
            return Err(OperationError::Transport {
                message: "malformed e2b process stream frame length".to_string(),
            });
        }

        let payload = &bytes[offset..offset + len];
        offset += len;

        if flags & 0x02 != 0 {
            parse_end_stream_payload(payload)?;
            continue;
        }
        if flags & 0x01 != 0 {
            return Err(OperationError::Unsupported {
                message: "compressed e2b process stream frames are not supported".to_string(),
            });
        }
        if payload.is_empty() {
            continue;
        }
        responses.push(
            serde_json::from_slice::<StartResponse>(payload).map_err(|error| {
                OperationError::Transport {
                    message: format!("failed to decode e2b process stream event: {error}"),
                }
            })?,
        );
    }

    collect_start_response(responses)
}

fn connect_envelope(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 5);
    framed.push(0);
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(payload);
    framed
}

fn collect_start_response<I>(responses: I) -> Result<E2bExecOutput, OperationError>
where
    I: IntoIterator<Item = StartResponse>,
{
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code = None;
    let mut end_error = None;

    for response in responses {
        match response.event {
            Some(ProcessEvent {
                data: Some(data), ..
            }) => {
                if let Some(chunk) = data.stdout {
                    stdout.extend(decode_stream_chunk("stdout", chunk.as_str())?);
                }
                if let Some(chunk) = data.stderr {
                    stderr.extend(decode_stream_chunk("stderr", chunk.as_str())?);
                }
                if let Some(chunk) = data.pty {
                    stdout.extend(decode_stream_chunk("pty", chunk.as_str())?);
                }
            }
            Some(ProcessEvent { end: Some(end), .. }) => {
                exit_code = end
                    .exit_code
                    .or_else(|| parse_status_exit_code(end.status.as_deref().unwrap_or_default()));
                end_error = end.error;
            }
            _ => {}
        }
    }

    if let Some(error) = end_error.filter(|value| !value.trim().is_empty()) {
        stderr.extend(error.as_bytes());
    }

    Ok(E2bExecOutput {
        stdout,
        stderr,
        exit_code,
        timed_out: false,
    })
}

fn decode_stream_chunk(label: &str, value: &str) -> Result<Vec<u8>, OperationError> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|error| OperationError::Transport {
            message: format!("failed to decode e2b process {label} chunk: {error}"),
        })
}

fn parse_status_exit_code(status: &str) -> Option<i32> {
    let status = status.trim();
    status
        .strip_prefix("exit status ")
        .and_then(|value| value.trim().parse::<i32>().ok())
        .or_else(|| {
            status
                .rsplit_once(' ')
                .and_then(|(_, tail)| tail.parse().ok())
        })
}

fn parse_end_stream_payload(payload: &[u8]) -> Result<(), OperationError> {
    if payload.is_empty() {
        return Ok(());
    }
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return Ok(());
    };
    if let Some(error) = value.get("error").filter(|value| !value.is_null()) {
        return Err(OperationError::Transport {
            message: format!("e2b process stream ended with error: {error}"),
        });
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct StartResponse {
    event: Option<ProcessEvent>,
}

#[derive(Debug, Deserialize)]
struct ProcessEvent {
    data: Option<DataEvent>,
    end: Option<EndEvent>,
}

#[derive(Debug, Deserialize)]
struct DataEvent {
    stdout: Option<String>,
    stderr: Option<String>,
    pty: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EndEvent {
    exit_code: Option<i32>,
    status: Option<String>,
    error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    #[test]
    fn parses_connect_start_stream() {
        let stdout = base64::engine::general_purpose::STANDARD.encode("hello\n");
        let event = format!(r#"{{"event":{{"data":{{"stdout":"{stdout}"}}}}}}"#);
        let end = r#"{"event":{"end":{"status":"exit status 0","exited":true}}}"#;
        let mut bytes = Vec::new();
        for payload in [event.as_bytes(), end.as_bytes()] {
            bytes.push(0);
            bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            bytes.extend_from_slice(payload);
        }

        let parsed = parse_start_stream(bytes.as_slice()).expect("parse stream");

        assert_eq!(parsed.stdout, b"hello\n");
        assert_eq!(parsed.exit_code, Some(0));
    }

    #[test]
    fn parses_status_exit_code() {
        assert_eq!(parse_status_exit_code("exit status 124"), Some(124));
    }
}
