use agent_contracts::backend::{capability::exec::ExecRequest, OperationError};
use tokio::process::Command;

pub(super) struct LocalCommandSpec {
    program: String,
    args: Vec<String>,
}

pub(super) fn build_command_spec(
    request: &ExecRequest,
) -> Result<LocalCommandSpec, OperationError> {
    if request.command.trim().is_empty() {
        return Err(OperationError::ExecutionFailed {
            message: "command cannot be empty".to_string(),
        });
    }

    if let Some(shell) = request.shell.as_deref() {
        if !request.args.is_empty() {
            return Err(OperationError::Unsupported {
                message: "shell execution does not support args".to_string(),
            });
        }
        return Ok(LocalCommandSpec {
            program: shell.to_string(),
            args: vec!["-c".to_string(), request.command.clone()],
        });
    }

    Ok(LocalCommandSpec {
        program: request.command.clone(),
        args: request.args.clone(),
    })
}

pub(super) fn command_from_spec(
    request: &ExecRequest,
    spec: LocalCommandSpec,
    policy: &crate::backends::local::policy::LocalBackendPolicy,
    cwd: Option<&std::path::Path>,
) -> Command {
    if let Some(profile) = policy.seatbelt_profile() {
        let mut command = Command::new("sandbox-exec");
        command.arg("-p").arg(profile.to_profile_text());
        command.arg(spec.program);
        command.args(spec.args);
        return command;
    }

    if let Some(cwd) = cwd {
        if let Some(args) = policy.linux_dynsandbox_args(cwd, request.extra.as_ref()) {
            let mut command = Command::new("dyn-sandbox");
            command.args(args);
            command.arg("--");
            command.arg(spec.program);
            command.args(spec.args);
            tracing::info!(
                "dyn-sandbox command built: {}",
                command
                    .as_std()
                    .get_args()
                    .map(|arg| arg.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            return command;
        }
        if let Some(args) = policy.bubblewrap_args(cwd) {
            let mut command = Command::new("bwrap");
            command.args(args);
            command.arg(spec.program);
            command.args(spec.args);
            return command;
        }
    }

    let mut command = Command::new(spec.program);
    command.args(spec.args);
    command
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../../../../../../tests/unit/operation_backend/backends/local/exec/command_spec_bubblewrap_test.rs"]
mod linux_bubblewrap_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "../../../../../../tests/unit/operation_backend/backends/local/exec/command_spec_dynsandbox_test.rs"]
mod linux_dynsandbox_tests;
