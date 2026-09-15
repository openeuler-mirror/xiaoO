use super::*;
use crate::backend::e2b::backend::E2bLifecycle;
use crate::backend::e2b::error::E2bFailureKind;
use agent_contracts::backend::BackendPath;
use base64::Engine;
use std::sync::Mutex;

fn exec_with_default_shell(default_shell: Option<&str>) -> E2bExec {
    E2bExec::new(Arc::new(E2bBackendState {
        backend_id: "e2b:test".to_string(),
        api_base: "https://api.e2b.test".to_string(),
        api_key: "test-key".to_string(),
        sandbox_id: "sandbox-test".to_string(),
        sandbox_domain: "e2b.test".to_string(),
        envd_access_token: None,
        envd_port: 49_983,
        envd_scheme: "https".to_string(),
        workspace_root: BackendPath::from_raw("/home/user/workspace".to_string()),
        home_dir: Some(BackendPath::from_raw("/home/user".to_string())),
        temp_root: BackendPath::from_raw("/tmp".to_string()),
        default_shell: default_shell.map(str::to_string),
        username: None,
        envd_file_upload_multipart: false,
        http: reqwest::Client::new(),
        lifecycle: Mutex::new(E2bLifecycle::Active),
    }))
}

fn exec_request(command: &str) -> ExecRequest {
    ExecRequest {
        command: command.to_string(),
        args: Vec::new(),
        cwd: Some(BackendPath::from_raw("/home/user/workspace".to_string())),
        ..Default::default()
    }
}

#[test]
fn direct_process_preserves_args_despite_configured_default_shell() {
    let exec = exec_with_default_shell(Some("/bin/sh"));
    let mut request = exec_request("rg");
    request.args = vec!["W\\s*=".to_string(), ".".to_string()];
    request.timeout_ms = Some(2_500);
    request.env = Some(vec![("LC_ALL".to_string(), "C".to_string())]);

    let (process, timeout_ms) = exec
        .build_start_process(request)
        .expect("direct request should be supported");

    assert_eq!(exec.default_shell(), Some("/bin/sh"));
    assert_eq!(timeout_ms, Some(2_500));
    assert_eq!(process.cmd, "timeout");
    assert_eq!(
        process.args,
        ["--signal=TERM", "2.500s", "rg", "W\\s*=", "."]
    );
    assert_eq!(process.cwd.as_deref(), Some("/home/user/workspace"));
    assert_eq!(process.env.get("LC_ALL").map(String::as_str), Some("C"));
    assert_eq!(process.connect_timeout_ms, Some(12_500));
}

#[test]
fn explicit_shell_builds_shell_command() {
    let exec = exec_with_default_shell(Some("/bin/sh"));
    let mut request = exec_request("printf shell-ok");
    request.shell = Some("/bin/bash".to_string());

    let (process, timeout_ms) = exec
        .build_start_process(request)
        .expect("explicit shell request should be supported");

    assert_eq!(timeout_ms, None);
    assert_eq!(process.cmd, "/bin/bash");
    assert_eq!(process.args, ["-c", "printf shell-ok"]);
}

#[test]
fn explicit_shell_rejects_process_args() {
    let exec = exec_with_default_shell(Some("/bin/sh"));
    let mut request = exec_request("printf");
    request.shell = Some("/bin/bash".to_string());
    request.args = vec!["shell-ok".to_string()];

    let error = match exec.build_start_process(request) {
        Ok(_) => panic!("shell request with args should fail"),
        Err(error) => error,
    };
    assert!(matches!(error, OperationError::Unsupported { .. }));
    assert_eq!(
        error.to_string(),
        "unsupported operation: shell execution does not support args"
    );
}

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

impl super::E2bExecFailure {
    pub(crate) fn retryable_for_test(message: &str) -> Self {
        Self::transport(E2bFailure::body_interrupted(message))
    }
}

fn parse_start_stream(bytes: &[u8]) -> Result<E2bExecOutput, OperationError> {
    let mut decoder = ConnectStartStreamDecoder::default();
    decoder
        .push(bytes)
        .and_then(|()| decoder.finish())
        .map_err(|failure| OperationError::Transport {
            message: failure.message,
        })
}
