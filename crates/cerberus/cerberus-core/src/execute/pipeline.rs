//! Execution pipeline orchestration.
//!
//! This module orchestrates the execution flow from preflight validation
//! through process spawning and result collection. The pipeline integrates
//! sandbox isolation when available on the platform.
//!
//! # Pipeline Stages
//!
//! 1. **Preflight**: Validate the request
//! 2. **Sandbox Setup**: Configure isolation (Linux only)
//! 3. **Execution**: Spawn and run the process
//! 4. **Collection**: Gather output and exit status
//!
//! # Sandbox Integration
//!
//! On Linux, the pipeline integrates sandbox isolation using a fork/exec path.
//! The child applies `SandboxSetup::setup()` before `exec`, while the parent
//! continues to collect stdout, stderr, exit status, timeout, and signal data.

use crate::audit::{ExecutionContext, TimeoutEvent};
use crate::error::{CerberusError, ExecutionError};
use crate::policy::Policy;
use crate::request::{ExecRequest, OutputPolicy, StdinPolicy};
use crate::result::{ExecMetadata, ExecResult};
use std::process::{Command, Stdio};
use std::time::{Instant, SystemTime};

pub(super) mod preflight {
    pub use super::super::preflight::validate;
}

/// Runs the execution pipeline for a validated request.
///
/// This is the core orchestration function that:
/// 1. Validates the request (preflight)
/// 2. Sets up sandbox isolation (Linux only)
/// 3. Spawns the process
/// 4. Collects output and exit status
///
/// # Errors
///
/// Returns `CerberusError` for:
/// - Request validation failures (`Request` variant)
/// - Sandbox setup failures (`SandboxSetup` variant)
/// - Process execution failures (`Execution` variant)
pub fn run_pipeline(request: ExecRequest, policy: &Policy) -> Result<ExecResult, CerberusError> {
    run_pipeline_with_context(request, policy, None)
}

fn emit_failed_and_return<T>(
    context: Option<&ExecutionContext>,
    error: CerberusError,
) -> Result<T, CerberusError> {
    if let Some(ctx) = context {
        ctx.emit_failed(&error.to_string())?;
    }

    Err(error)
}

/// Runs the execution pipeline with an optional audit context.
pub(crate) fn run_pipeline_with_context(
    request: ExecRequest,
    policy: &Policy,
    mut context: Option<&mut ExecutionContext>,
) -> Result<ExecResult, CerberusError> {
    if let Some(ctx) = context.as_deref() {
        ctx.emit_request_received()?;
    }

    if let Err(error) = preflight::validate(&request) {
        return emit_failed_and_return(context.as_deref(), error.into());
    }

    let filtered_request = match super::env_filter::apply_env_filtering(request, policy) {
        Ok(filtered_request) => filtered_request,
        Err(error) => return emit_failed_and_return(context.as_deref(), error),
    };

    if let Some(ctx) = context.as_deref_mut() {
        ctx.start()?;
    }

    let result = match execute_process(&filtered_request, policy, context.as_deref()) {
        Ok(result) => result,
        Err(error) => return emit_failed_and_return(context.as_deref(), error),
    };

    if let Some(ctx) = context.as_deref() {
        if result.metadata.timed_out {
            return Ok(result);
        }
        ctx.emit_completed(&result)?;
    }

    Ok(result)
}

fn execute_process(
    request: &ExecRequest,
    policy: &Policy,
    context: Option<&ExecutionContext>,
) -> Result<ExecResult, CerberusError> {
    #[cfg(target_os = "linux")]
    {
        if !should_use_linux_sandbox(policy) {
            return execute_process_direct(request, policy, context);
        }

        super::linux::execute_process_linux(request, policy, context)
    }

    #[cfg(not(target_os = "linux"))]
    {
        execute_process_direct(request, policy, context)
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn should_use_linux_sandbox(policy: &Policy) -> bool {
    policy.namespaces.requires_linux_namespaces()
        || !policy.fs_rules().is_empty()
        || policy.resources.max_memory_bytes.is_some()
        || policy.resources.max_processes.is_some()
        || (policy.allow_network()
            && policy
                .network_policy
                .as_ref()
                .map(|np| np.is_enabled())
                .unwrap_or(false))
}

fn execute_process_direct(
    request: &ExecRequest,
    policy: &Policy,
    context: Option<&ExecutionContext>,
) -> Result<ExecResult, CerberusError> {
    let start = Instant::now();

    let mut cmd = Command::new(&request.program);
    cmd.args(&request.args);

    if let Some(ref cwd) = request.cwd {
        cmd.current_dir(cwd);
    }

    cmd.env_clear();
    for (key, value) in &request.env {
        cmd.env(key, value);
    }

    match &request.stdin {
        StdinPolicy::Inherit => {
            cmd.stdin(Stdio::inherit());
        }
        StdinPolicy::Null => {
            cmd.stdin(Stdio::null());
        }
        StdinPolicy::Bytes(_) => {
            cmd.stdin(Stdio::piped());
        }
    }

    match &request.stdout {
        OutputPolicy::Inherit => {
            cmd.stdout(Stdio::inherit());
        }
        OutputPolicy::Capture => {
            cmd.stdout(Stdio::piped());
        }
    }

    match &request.stderr {
        OutputPolicy::Inherit => {
            cmd.stderr(Stdio::inherit());
        }
        OutputPolicy::Capture => {
            cmd.stderr(Stdio::piped());
        }
    }

    let mut child = cmd.spawn().map_err(|e| {
        ExecutionError::SpawnFailed(format!("Failed to spawn '{}': {}", request.program, e))
    })?;
    let child_pid = child.id();

    if let StdinPolicy::Bytes(data) = &request.stdin {
        use std::io::Write;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(data).map_err(|e| {
                ExecutionError::SpawnFailed(format!("Failed to write stdin: {}", e))
            })?;
        }
    }

    let timeout = policy.timeout();
    let exit_status = if timeout.as_secs() > 0 {
        match child.wait_timeout(timeout) {
            Ok(Some(status)) => status,
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let duration = start.elapsed();
                if let Some(ctx) = context {
                    ctx.emit_timeout(&TimeoutEvent {
                        duration: timeout,
                        pid: child_pid,
                        timestamp: SystemTime::now(),
                    })?;
                }
                return Ok(ExecResult::new(-1)
                    .duration(duration)
                    .metadata(ExecMetadata::new().timed_out().killed()));
            }
            Err(e) => {
                return Err(ExecutionError::WaitFailed(e.to_string()).into());
            }
        }
    } else {
        child
            .wait()
            .map_err(|e| ExecutionError::WaitFailed(e.to_string()))?
    };

    let duration = start.elapsed();

    let stdout_bytes = match child.stdout.take() {
        Some(mut stdout) => {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stdout, &mut buf);
            buf
        }
        None => Vec::new(),
    };
    let stderr_bytes = match child.stderr.take() {
        Some(mut stderr) => {
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut stderr, &mut buf);
            buf
        }
        None => Vec::new(),
    };

    let exit_code = exit_status.code().unwrap_or(-1);

    #[cfg(unix)]
    let metadata = {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = exit_status.signal() {
            ExecMetadata::new().killed().signal(signal)
        } else {
            ExecMetadata::new()
        }
    };

    #[cfg(not(unix))]
    let metadata = ExecMetadata::new();

    Ok(ExecResult::new(exit_code)
        .stdout(stdout_bytes)
        .stderr(stderr_bytes)
        .duration(duration)
        .metadata(metadata))
}

trait ChildWaitTimeout {
    fn wait_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>>;
}

impl ChildWaitTimeout for std::process::Child {
    fn wait_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> std::io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            match self.try_wait()? {
                Some(status) => return Ok(Some(status)),
                None => {
                    if start.elapsed() >= timeout {
                        return Ok(None);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::error::SandboxSetupError;
    #[cfg(target_os = "linux")]
    use std::sync::{Mutex, OnceLock};

    #[cfg(target_os = "linux")]
    fn linux_sandbox_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn direct_execution_test_policy() -> Policy {
        let mut policy = Policy::minimal();
        policy.path_groups = crate::policy::PathGroups::default();
        policy.custom_paths.clear();
        policy.namespaces.mount = false;
        policy.resources.max_memory_bytes = None;
        policy.resources.max_processes = None;
        policy
    }

    #[test]
    fn run_pipeline_simple_command() {
        let request = ExecRequest::new("echo").arg("hello").capture_output();
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello"));
    }

    #[test]
    fn run_pipeline_command_with_cwd() {
        let request = ExecRequest::new("ls").cwd("/tmp");
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
    }

    #[test]
    fn run_pipeline_command_with_env() {
        let request = ExecRequest::new("sh")
            .arg("-c")
            .arg("echo $HOME")
            .env("HOME", "/test/home")
            .capture_output();
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("/test/home"));
    }

    #[test]
    fn run_pipeline_env_filtering_blocks_non_whitelisted() {
        let request = ExecRequest::new("sh")
            .arg("-c")
            .arg("printf '%s' \"${MY_VAR:-missing}\"")
            .env("MY_VAR", "secret_value")
            .capture_output();
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert_eq!(result.stdout_utf8(), "missing");
    }

    #[test]
    fn run_pipeline_nonexistent_command() {
        let request = ExecRequest::new("/nonexistent/command/xyz");
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy);

        assert!(result.is_err());
    }

    #[test]
    fn run_pipeline_command_exits_nonzero() {
        let request = ExecRequest::new("sh").arg("-c").arg("exit 42");
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert_eq!(result.exit_code, 42);
        assert!(!result.is_success());
    }

    #[test]
    fn run_pipeline_with_stdin_bytes() {
        let request = ExecRequest::new("cat")
            .stdin(StdinPolicy::Bytes(b"hello from stdin".to_vec()))
            .capture_output();
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout_utf8().contains("hello from stdin"));
    }

    #[test]
    fn run_pipeline_with_null_stdin() {
        let request = ExecRequest::new("cat")
            .stdin(StdinPolicy::Null)
            .capture_output();
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stdout.is_empty());
    }

    #[test]
    fn run_pipeline_command_with_stderr() {
        let request = ExecRequest::new("sh")
            .arg("-c")
            .arg("echo 'error message' >&2")
            .capture_output();
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.is_success());
        assert!(result.stderr_utf8().contains("error message"));
    }

    #[test]
    fn run_pipeline_duration_is_recorded() {
        let request = ExecRequest::new("echo").arg("test");
        let policy = direct_execution_test_policy();
        let result = run_pipeline(request, &policy).expect("execution should succeed");

        assert!(result.duration.as_nanos() > 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_pipeline_strict_fails_closed_when_namespace_gap_is_detected() {
        let _guard = linux_sandbox_test_lock().lock().unwrap();
        let request = ExecRequest::new("echo").arg("hello");
        let policy = Policy::strict();
        let caps = crate::sandbox::detect_capabilities();

        match run_pipeline(request, &policy) {
            Ok(result) => {
                assert!(caps.namespaces.user);
                assert!(caps.namespaces.pid);
                assert!(caps.namespaces.network);
                assert!(result.is_success());
            }
            Err(CerberusError::SandboxSetup(SandboxSetupError::CapabilityError {
                feature,
                reason,
            })) => {
                assert_eq!(feature, "namespaces");
                assert!(!caps.namespaces.user || !caps.namespaces.pid || !caps.namespaces.network);
                if !caps.namespaces.user {
                    assert!(reason.contains("user"));
                }
                if !caps.namespaces.pid {
                    assert!(reason.contains("pid"));
                }
                if !caps.namespaces.network {
                    assert!(reason.contains("network"));
                }
            }
            Err(error) => panic!("unexpected strict pipeline error: {error:?}"),
        }
    }

    /// Tests for environment allowlist enforcement.
    mod env_allowlist_tests {
        use super::*;
        use crate::policy::EnvironmentConfig;

        fn env_filter_test_policy() -> Policy {
            direct_execution_test_policy()
        }

        #[test]
        fn env_filtering_blocks_non_whitelisted_var() {
            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("printf '%s' \"${MY_SECRET:-missing}\"")
                .env("MY_SECRET", "secret_value")
                .capture_output();
            let policy = env_filter_test_policy();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert_eq!(result.stdout_utf8(), "missing");
        }

        #[test]
        fn env_filtering_minimal_allows_home() {
            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("printf '%s' \"$HOME\"")
                .env("HOME", "/home/user")
                .capture_output();
            let policy = env_filter_test_policy();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert!(result.stdout_utf8().contains("/home/user"));
        }

        #[test]
        fn env_filtering_restricted_whitelist_blocks_home() {
            let mut policy = env_filter_test_policy();
            policy.environment = EnvironmentConfig {
                whitelist: vec!["PATH".to_string(), "LANG".to_string()],
            };

            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("printf '%s' \"${HOME:-missing}\"")
                .env("HOME", "/home/user")
                .capture_output();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert_eq!(result.stdout_utf8(), "missing");
        }

        #[test]
        fn env_filtering_restricted_whitelist_blocks_custom_var() {
            let mut policy = env_filter_test_policy();
            policy.environment = EnvironmentConfig {
                whitelist: vec!["PATH".to_string(), "LANG".to_string()],
            };

            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("printf '%s' \"${SECRET:-missing}\"")
                .env("SECRET", "topsecret")
                .capture_output();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert_eq!(result.stdout_utf8(), "missing");
        }

        #[test]
        fn env_filtering_restricted_whitelist_allows_path() {
            let mut policy = env_filter_test_policy();
            policy.environment = EnvironmentConfig {
                whitelist: vec!["PATH".to_string(), "LANG".to_string()],
            };

            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("printf '%s' \"$PATH\"")
                .env("PATH", "/usr/bin")
                .capture_output();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert!(result.stdout_utf8().contains("/usr/bin"));
        }

        #[test]
        fn env_filtering_preserves_existing_env_vars() {
            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("printf '%s' \"${CARGO_MANIFEST_DIR:-missing}\"")
                .capture_output();
            let policy = env_filter_test_policy();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert_eq!(result.stdout_utf8(), "missing");
        }
    }

    /// Linux-only tests for sandbox enforcement.
    #[cfg(target_os = "linux")]
    mod linux_sandbox_tests {
        use super::*;
        use crate::policy::PathGroups;

        #[test]
        fn with_network_profile_requires_pid_namespace_support() {
            let _guard = linux_sandbox_test_lock().lock().unwrap();
            let request = ExecRequest::new("/usr/bin/printf")
                .arg("%s")
                .arg("ok")
                .capture_output();
            let policy = Policy::with_network();
            let caps = crate::sandbox::detect_capabilities();

            match run_pipeline(request, &policy) {
                Ok(result) => {
                    assert!(caps.namespaces.pid);
                    assert!(result.is_success());
                    assert_eq!(result.stdout_utf8(), "ok");
                }
                Err(CerberusError::SandboxSetup(SandboxSetupError::CapabilityError {
                    feature,
                    reason,
                })) => {
                    assert_eq!(feature, "namespaces");
                    assert!(!caps.namespaces.pid);
                    assert!(reason.contains("pid"));
                }
                Err(error) => panic!("unexpected with_network pipeline error: {error:?}"),
            }
        }

        #[test]
        fn minimal_profile_uses_linux_sandbox_for_filesystem_and_resource_controls() {
            let _guard = linux_sandbox_test_lock().lock().unwrap();
            assert!(should_use_linux_sandbox(&Policy::minimal()));
        }

        #[test]
        fn resource_limits_alone_require_linux_sandbox() {
            let mut policy = Policy::minimal();
            policy.path_groups = PathGroups::default();
            policy.custom_paths.clear();
            policy.namespaces.mount = false;
            policy.resources.max_memory_bytes = Some(64 * 1024 * 1024);
            policy.resources.max_processes = Some(4);

            assert!(should_use_linux_sandbox(&policy));
        }
    }

    /// Cross-platform tests that should work on all platforms.
    mod cross_platform_tests {
        use super::*;

        #[test]
        fn minimal_profile_executes_simple_command() {
            let request = ExecRequest::new("echo").arg("hello").capture_output();
            let policy = direct_execution_test_policy();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.is_success());
            assert!(result.stdout_utf8().contains("hello"));
        }

        #[test]
        fn exit_code_propagates_correctly() {
            let request = ExecRequest::new("sh").arg("-c").arg("exit 42");
            let policy = direct_execution_test_policy();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert_eq!(result.exit_code, 42);
        }

        #[test]
        fn stderr_is_captured() {
            let request = ExecRequest::new("sh")
                .arg("-c")
                .arg("echo 'error' >&2")
                .capture_output();
            let policy = direct_execution_test_policy();
            let result = run_pipeline(request, &policy).expect("execution should succeed");

            assert!(result.stderr_utf8().contains("error"));
        }
    }
}
