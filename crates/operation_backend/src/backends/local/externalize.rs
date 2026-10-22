//! Mutation externalization: when a plugin-provided `spawn_wrapper`
//! instruction sets `externalize_fs_writes`, mutation-class filesystem
//! requests run through a `[wrapper.prefix...] xiaoo-backend-cli request
//! --stdin` subprocess fed a JSON envelope over stdin (base64 content, no
//! argv limits); the subprocess rebuilds an identical backend from the
//! envelope's backend section. Fail-loud by design: a failure to spawn or
//! run the subprocess is an error, never a silent in-process write that
//! would mis-attribute the mutation.

use crate::backends::local::spawn_wrapper::SpawnWrapper;
use agent_contracts::backend::OperationError;
use serde_json::{json, Value};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

const BACKEND_CLI_NAME: &str = "xiaoo-backend-cli";

/// Locate the `xiaoo-backend-cli` binary: the `XIAOO_BACKEND_CLI` override
/// (fail loud if set but missing), else beside the current executable, else
/// `PATH` lookup.
pub(crate) fn resolve_backend_cli_path() -> Result<PathBuf, OperationError> {
    if let Some(override_path) = std::env::var_os("XIAOO_BACKEND_CLI") {
        let path = PathBuf::from(override_path);
        if path.is_file() {
            return Ok(path);
        }
        return Err(missing_cli_error(format!(
            "XIAOO_BACKEND_CLI points to a missing file: {}",
            path.display()
        )));
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(directory) = current_exe.parent() {
            let candidate = directory.join(BACKEND_CLI_NAME);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    if let Some(path_var) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path_var) {
            let candidate = directory.join(BACKEND_CLI_NAME);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    Err(missing_cli_error(format!(
        "'{BACKEND_CLI_NAME}' was not found beside the current executable or on PATH"
    )))
}

fn missing_cli_error(detail: String) -> OperationError {
    OperationError::ExecutionFailed {
        message: format!(
            "failed to locate the backend CLI needed to externalize this write: {detail}"
        ),
    }
}

/// Build the request envelope and run it through
/// `[wrapper.prefix...] <cli> request --stdin`. Returns the subprocess's
/// parsed stdout JSON (the request result).
pub(crate) async fn run_externalized(
    wrapper: &SpawnWrapper,
    source_config: &Value,
    request_kind: &str,
    request_args: Value,
) -> Result<Value, OperationError> {
    let cli_path = resolve_backend_cli_path()?;
    let envelope = json!({
        "backend": source_config,
        "request": { "kind": request_kind, "args": request_args },
    });
    let envelope_bytes =
        serde_json::to_vec(&envelope).map_err(|error| OperationError::ExecutionFailed {
            message: format!("failed to serialize externalized request envelope: {error}"),
        })?;

    let mut command = Command::new(&wrapper.prefix[0]);
    command.args(&wrapper.prefix[1..]);
    command.arg(&cli_path);
    command.arg("request").arg("--stdin");
    command.stdin(std::process::Stdio::piped());
    command.stdout(std::process::Stdio::piped());
    command.stderr(std::process::Stdio::piped());
    // Same threat model as the exec path: the wrapper is plugin-contributed
    // code and must not see the daemon's credential-laden environment.
    crate::backends::local::spawn_env::scrub_child_environment(&mut command);
    // A timeout drops this future; the whole spawned tree must die with it —
    // a surviving CLI (a grandchild under a non-exec'ing wrapper) would
    // complete the mutation after the caller already saw a timeout.
    // kill_on_drop covers the direct child; the guard below kills the unix
    // process group.
    command.kill_on_drop(true);
    #[cfg(unix)]
    {
        command.process_group(0);
    }

    let mut child = process_coordination::spawn_tokio(&mut command)
        .await
        .map_err(|error| OperationError::ExecutionFailed {
            message: format!(
                "failed to spawn externalized request ({} request --stdin): {error}",
                cli_path.display()
            ),
        })?;

    // Global registration lets daemon-exit cleanup reap a leaked group; the
    // guard kills the group if the future is dropped before completion.
    #[cfg(unix)]
    let group = ProcessGroupKillGuard::arm(child.id().unwrap_or(0) as i32);

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&envelope_bytes).await.map_err(|error| {
            OperationError::ExecutionFailed {
                message: format!("failed to write externalized request envelope to stdin: {error}"),
            }
        })?;
    }

    let output =
        child
            .wait_with_output()
            .await
            .map_err(|error| OperationError::ExecutionFailed {
                message: format!("failed to await externalized request subprocess: {error}"),
            })?;

    // Request completed and the child was reaped: nothing left to clean up.
    #[cfg(unix)]
    group.disarm();

    if !output.status.success() {
        // Restore the typed error so e.g. `SandboxPolicyDenied` keeps
        // driving the daemon's permission flow.
        if let Some(error) = decode_cli_error(&output.stdout) {
            return Err(error);
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(OperationError::ExecutionFailed {
            message: format!(
                "externalized '{}' request failed with {}{}",
                request_kind,
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            ),
        });
    }

    serde_json::from_slice(&output.stdout).map_err(|error| OperationError::ExecutionFailed {
        message: format!(
            "externalized '{request_kind}' request returned invalid JSON output: {error}"
        ),
    })
}

/// Decode the CLI's structured error document (`{"error": ...}` on stdout)
/// back into a typed [`OperationError`]; `None` when stdout is not such a
/// document, so the caller falls back to the stderr-text error.
fn decode_cli_error(stdout: &[u8]) -> Option<OperationError> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    let error = value.get("error")?;
    serde_json::from_value(error.clone()).ok()
}

/// Kills the spawned process group when dropped before being disarmed, so a
/// timeout/cancel cannot leave the CLI running as a wrapper's grandchild.
#[cfg(unix)]
struct ProcessGroupKillGuard {
    pgid: i32,
    armed: bool,
}

#[cfg(unix)]
impl ProcessGroupKillGuard {
    fn arm(pgid: i32) -> Self {
        crate::process_group::register_pgid(pgid);
        Self { pgid, armed: true }
    }

    /// Disarm the group kill and drop the global registration.
    fn disarm(mut self) {
        self.armed = false;
        crate::process_group::unregister_pgid(self.pgid);
    }
}

#[cfg(unix)]
impl Drop for ProcessGroupKillGuard {
    fn drop(&mut self) {
        if self.armed {
            crate::process_group::send_sigkill_to_group(self.pgid);
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/operation_backend/backends/local/externalize_test.rs"]
mod tests;
