use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    ExecutionState, OperationError,
};
use async_trait::async_trait;
use base64::Engine;
use futures::StreamExt;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::backend::{parse_error_message, E2bBackendState, DEFAULT_SHELL};
use super::error::E2bFailure;

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
        self.run_shell_script_detailed(script, cwd)
            .await
            .map_err(E2bExecFailure::into_operation_error)
    }

    pub(crate) async fn run_shell_script_detailed(
        &self,
        script: &str,
        cwd: Option<&str>,
    ) -> Result<E2bExecOutput, E2bExecFailure> {
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

#[derive(Debug)]
pub(crate) struct E2bExecOutput {
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
}

#[derive(Debug)]
pub(crate) struct E2bExecFailure {
    failure: E2bFailure,
    operation_error: Option<OperationError>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    execution_state: Option<ExecutionState>,
}

impl E2bExecFailure {
    fn transport(failure: E2bFailure) -> Self {
        Self {
            failure,
            operation_error: None,
            stdout: Vec::new(),
            stderr: Vec::new(),
            execution_state: None,
        }
    }

    fn operation(failure: E2bFailure, operation_error: OperationError) -> Self {
        Self {
            failure,
            operation_error: Some(operation_error),
            stdout: Vec::new(),
            stderr: Vec::new(),
            execution_state: None,
        }
    }

    fn interrupted(failure: E2bFailure, stdout: Vec<u8>, stderr: Vec<u8>) -> Self {
        Self {
            failure,
            operation_error: None,
            stdout,
            stderr,
            execution_state: Some(ExecutionState::RunningOrCompleted),
        }
    }

    pub(crate) fn retryable(&self) -> bool {
        self.failure.retryable
    }

    pub(crate) fn message(&self) -> &str {
        self.failure.message.as_str()
    }

    pub(crate) fn into_operation_error(self) -> OperationError {
        if let Some(error) = self.operation_error {
            return error;
        }
        if let Some(state) = self.execution_state {
            return OperationError::ExecutionInterrupted {
                message: self.failure.message,
                stdout: self.stdout,
                stderr: self.stderr,
                state,
            };
        }
        OperationError::Transport {
            message: self.failure.message,
        }
    }

    #[cfg(test)]
    pub(crate) fn retryable_for_test(message: &str) -> Self {
        Self::transport(E2bFailure::body_interrupted(message))
    }
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
        .await
        .map_err(E2bExecFailure::into_operation_error)?;
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
) -> Result<E2bExecOutput, E2bExecFailure> {
    state.ensure_active().map_err(|error| {
        E2bExecFailure::operation(E2bFailure::protocol(error.to_string()), error)
    })?;
    let body = json!({
        "process": {
            "cmd": request.cmd,
            "args": request.args,
            "envs": request.env,
            "cwd": request.cwd,
        },
        "pty": null,
        "stdin": false,
        "tag": format!("xiaoo-{}", uuid::Uuid::new_v4()),
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

    let started_at = Instant::now();
    let response = builder.send().await.map_err(|error| {
        let failure = E2bFailure::from_reqwest("failed to start e2b process", &error);
        failure.log(
            "start_process",
            Some(state.sandbox_id.as_str()),
            None,
            started_at.elapsed().as_millis(),
        );
        E2bExecFailure::transport(failure)
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let message = parse_error_message(text.as_str()).unwrap_or(text);
        let failure = E2bFailure::from_status("start e2b process", status, message.clone());
        failure.log(
            "start_process",
            Some(state.sandbox_id.as_str()),
            None,
            started_at.elapsed().as_millis(),
        );
        let operation_error = match status {
            StatusCode::NOT_FOUND => OperationError::NotFound { path: message },
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                OperationError::PermissionDenied { path: message }
            }
            _ => OperationError::Transport {
                message: failure.message.clone(),
            },
        };
        return Err(E2bExecFailure::operation(failure, operation_error));
    }

    let mut stream = response.bytes_stream();
    let mut decoder = ConnectStartStreamDecoder::default();
    while let Some(next) = stream.next().await {
        match next {
            Ok(chunk) => {
                if let Err(failure) = decoder.push(chunk.as_ref()) {
                    failure.log(
                        "read_process_stream",
                        Some(state.sandbox_id.as_str()),
                        None,
                        started_at.elapsed().as_millis(),
                    );
                    if decoder.completed() {
                        tracing::warn!(
                            operation = "read_process_stream",
                            sandbox_id = %state.sandbox_id,
                            elapsed_ms = started_at.elapsed().as_millis(),
                            "E2B process completed before a trailing stream protocol error"
                        );
                        return Ok(decoder.into_output());
                    }
                    return Err(decoder.into_interrupted(failure));
                }
            }
            Err(error) => {
                let failure = E2bFailure::from_reqwest("failed to read e2b process stream", &error);
                failure.log(
                    "read_process_stream",
                    Some(state.sandbox_id.as_str()),
                    None,
                    started_at.elapsed().as_millis(),
                );
                if decoder.completed() {
                    tracing::warn!(
                        operation = "read_process_stream",
                        sandbox_id = %state.sandbox_id,
                        elapsed_ms = started_at.elapsed().as_millis(),
                        "E2B process completed before its response stream was interrupted"
                    );
                    return Ok(decoder.into_output());
                }
                return Err(decoder.into_interrupted(failure));
            }
        }
    }

    match decoder.finish() {
        Ok(output) => Ok(output),
        Err(failure) => {
            failure.log(
                "read_process_stream",
                Some(state.sandbox_id.as_str()),
                None,
                started_at.elapsed().as_millis(),
            );
            if decoder.completed() {
                tracing::warn!(
                    operation = "read_process_stream",
                    sandbox_id = %state.sandbox_id,
                    elapsed_ms = started_at.elapsed().as_millis(),
                    "E2B process completed before its trailing stream frame was truncated"
                );
                return Ok(decoder.into_output());
            }
            Err(decoder.into_interrupted(failure))
        }
    }
}

#[cfg(test)]
fn parse_start_stream(bytes: &[u8]) -> Result<E2bExecOutput, OperationError> {
    let mut decoder = ConnectStartStreamDecoder::default();
    decoder
        .push(bytes)
        .and_then(|()| decoder.finish())
        .map_err(|failure| OperationError::Transport {
            message: failure.message,
        })
}

fn connect_envelope(payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(payload.len() + 5);
    framed.push(0);
    framed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    framed.extend_from_slice(payload);
    framed
}

#[derive(Default)]
struct StartStreamCollector {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
    completed: bool,
}

impl StartStreamCollector {
    fn apply(&mut self, response: StartResponse) -> Result<(), E2bFailure> {
        match response.event {
            Some(ProcessEvent {
                data: Some(data), ..
            }) => {
                if let Some(chunk) = data.stdout {
                    self.stdout
                        .extend(decode_stream_chunk("stdout", chunk.as_str())?);
                }
                if let Some(chunk) = data.stderr {
                    self.stderr
                        .extend(decode_stream_chunk("stderr", chunk.as_str())?);
                }
                if let Some(chunk) = data.pty {
                    self.stdout
                        .extend(decode_stream_chunk("pty", chunk.as_str())?);
                }
            }
            Some(ProcessEvent { end: Some(end), .. }) => {
                self.exit_code = end
                    .exit_code
                    .or_else(|| parse_status_exit_code(end.status.as_deref().unwrap_or_default()));
                if let Some(error) = end.error.filter(|value| !value.trim().is_empty()) {
                    self.stderr.extend(error.as_bytes());
                }
                self.completed = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn into_output(self) -> E2bExecOutput {
        E2bExecOutput {
            stdout: self.stdout,
            stderr: self.stderr,
            exit_code: self.exit_code,
            timed_out: false,
        }
    }
}

#[derive(Default)]
struct ConnectStartStreamDecoder {
    mode: Option<StreamMode>,
    buffer: Vec<u8>,
    collector: StartStreamCollector,
}

#[derive(Clone, Copy)]
enum StreamMode {
    Framed,
    PlainJson,
}

impl ConnectStartStreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<(), E2bFailure> {
        self.buffer.extend_from_slice(bytes);
        if self.mode.is_none() {
            let Some(first) = self
                .buffer
                .iter()
                .copied()
                .find(|byte| !byte.is_ascii_whitespace())
            else {
                return Ok(());
            };
            self.mode = Some(if first == b'{' {
                StreamMode::PlainJson
            } else {
                StreamMode::Framed
            });
        }
        if matches!(self.mode, Some(StreamMode::Framed)) {
            self.drain_frames()?;
        }
        Ok(())
    }

    fn drain_frames(&mut self) -> Result<(), E2bFailure> {
        loop {
            if self.buffer.len() < 5 {
                return Ok(());
            }
            let flags = self.buffer[0];
            let len = u32::from_be_bytes([
                self.buffer[1],
                self.buffer[2],
                self.buffer[3],
                self.buffer[4],
            ]) as usize;
            if self.buffer.len().saturating_sub(5) < len {
                return Ok(());
            }
            let payload = self.buffer[5..5 + len].to_vec();
            self.buffer.drain(..5 + len);
            if flags & 0x02 != 0 {
                parse_end_stream_payload(payload.as_slice())?;
                continue;
            }
            if flags & 0x01 != 0 {
                return Err(E2bFailure::protocol(
                    "compressed e2b process stream frames are not supported",
                ));
            }
            if payload.is_empty() {
                continue;
            }
            let response =
                serde_json::from_slice::<StartResponse>(payload.as_slice()).map_err(|error| {
                    E2bFailure::protocol(format!(
                        "failed to decode e2b process stream event: {error}"
                    ))
                })?;
            self.collector.apply(response)?;
        }
    }

    fn finish(&mut self) -> Result<E2bExecOutput, E2bFailure> {
        match self.mode {
            Some(StreamMode::PlainJson) => {
                let response = serde_json::from_slice::<StartResponse>(self.buffer.as_slice())
                    .map_err(|error| {
                        E2bFailure::protocol(format!(
                            "failed to decode e2b process response: {error}"
                        ))
                    })?;
                self.collector.apply(response)?;
                self.buffer.clear();
            }
            Some(StreamMode::Framed) if !self.buffer.is_empty() => {
                let message = if self.buffer.len() < 5 {
                    "malformed e2b process stream frame header"
                } else {
                    "malformed e2b process stream frame length"
                };
                return Err(E2bFailure::body_interrupted(message));
            }
            None => return Err(E2bFailure::body_interrupted("empty e2b process stream")),
            Some(StreamMode::Framed) => {}
        }
        Ok(std::mem::take(&mut self.collector).into_output())
    }

    fn completed(&self) -> bool {
        self.collector.completed
    }

    fn into_output(self) -> E2bExecOutput {
        self.collector.into_output()
    }

    fn into_interrupted(self, failure: E2bFailure) -> E2bExecFailure {
        E2bExecFailure::interrupted(failure, self.collector.stdout, self.collector.stderr)
    }
}

fn decode_stream_chunk(label: &str, value: &str) -> Result<Vec<u8>, E2bFailure> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|error| {
            E2bFailure::protocol(format!(
                "failed to decode e2b process {label} chunk: {error}"
            ))
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

fn parse_end_stream_payload(payload: &[u8]) -> Result<(), E2bFailure> {
    if payload.is_empty() {
        return Ok(());
    }
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return Ok(());
    };
    if let Some(error) = value.get("error").filter(|value| !value.is_null()) {
        return Err(E2bFailure::retryable_protocol(format!(
            "e2b process stream ended with error: {error}"
        )));
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
    use crate::backend::e2b::error::E2bFailureKind;
    use base64::Engine;

    fn framed_payloads(payloads: &[&[u8]]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for payload in payloads {
            bytes.push(0);
            bytes.extend_from_slice(&(payload.len() as u32).to_be_bytes());
            bytes.extend_from_slice(payload);
        }
        bytes
    }

    #[test]
    fn parses_connect_start_stream() {
        let stdout = base64::engine::general_purpose::STANDARD.encode("hello\n");
        let event = format!(r#"{{"event":{{"data":{{"stdout":"{stdout}"}}}}}}"#);
        let end = r#"{"event":{"end":{"status":"exit status 0","exited":true}}}"#;
        let bytes = framed_payloads(&[event.as_bytes(), end.as_bytes()]);

        let parsed = parse_start_stream(bytes.as_slice()).expect("parse stream");

        assert_eq!(parsed.stdout, b"hello\n");
        assert_eq!(parsed.exit_code, Some(0));
    }

    #[test]
    fn parses_frames_split_at_every_chunk_boundary() {
        let stdout = base64::engine::general_purpose::STANDARD.encode("split output");
        let event = format!(r#"{{"event":{{"data":{{"stdout":"{stdout}"}}}}}}"#);
        let end = r#"{"event":{"end":{"exitCode":0}}}"#;
        let bytes = framed_payloads(&[event.as_bytes(), end.as_bytes()]);

        for split in 1..bytes.len() {
            let mut decoder = ConnectStartStreamDecoder::default();
            decoder.push(&bytes[..split]).expect("first chunk");
            decoder.push(&bytes[split..]).expect("second chunk");
            let output = decoder.finish().expect("complete stream");
            assert_eq!(output.stdout, b"split output", "split at {split}");
            assert_eq!(output.exit_code, Some(0), "split at {split}");
        }
    }

    #[test]
    fn preserves_partial_output_on_truncated_frame() {
        let stdout = base64::engine::general_purpose::STANDARD.encode("partial");
        let event = format!(r#"{{"event":{{"data":{{"stdout":"{stdout}"}}}}}}"#);
        let mut bytes = framed_payloads(&[event.as_bytes()]);
        bytes.extend_from_slice(&[0, 0, 0, 0, 8, b'{']);

        let mut decoder = ConnectStartStreamDecoder::default();
        decoder.push(bytes.as_slice()).expect("first event parses");
        let failure = decoder.finish().expect_err("truncated frame fails");
        assert_eq!(failure.kind, E2bFailureKind::BodyInterrupted);
        assert_eq!(decoder.collector.stdout, b"partial");
    }

    #[test]
    fn interrupted_operation_error_carries_partial_output_and_state() {
        let stdout = base64::engine::general_purpose::STANDARD.encode("partial");
        let event = format!(r#"{{"event":{{"data":{{"stdout":"{stdout}"}}}}}}"#);
        let bytes = framed_payloads(&[event.as_bytes()]);
        let mut decoder = ConnectStartStreamDecoder::default();
        decoder.push(bytes.as_slice()).expect("event parses");

        let error = decoder
            .into_interrupted(E2bFailure::body_interrupted("connection reset"))
            .into_operation_error();

        match error {
            OperationError::ExecutionInterrupted {
                stdout,
                stderr,
                state,
                ..
            } => {
                assert_eq!(stdout, b"partial");
                assert!(stderr.is_empty());
                assert_eq!(state, ExecutionState::RunningOrCompleted);
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    #[test]
    fn parses_status_exit_code() {
        assert_eq!(parse_status_exit_code("exit status 124"), Some(124));
    }
}
