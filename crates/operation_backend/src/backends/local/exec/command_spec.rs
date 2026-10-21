use crate::backends::local::policy::LocalBackendPolicy;
use crate::backends::local::spawn_wrapper::spawn_wrapper_from_extra;
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
    policy: &LocalBackendPolicy,
    cwd: Option<&std::path::Path>,
) -> Command {
    // Inner (program, args): the plain tool command, or a sandbox launcher
    // wrapping it. Computed first so the optional plugin-provided spawn
    // wrapper can be prepended around whichever combination applies — the
    // wrapper is always the outermost layer (wrapper → sandbox → command).
    let inner = inner_program_args(request, &spec, policy, cwd);
    match spawn_wrapper_from_extra(request.extra.as_ref()) {
        Some(wrapper) => {
            let mut command = Command::new(&wrapper.prefix[0]);
            command.args(&wrapper.prefix[1..]);
            command.arg(inner.0);
            command.args(inner.1);
            command
        }
        None => {
            let mut command = Command::new(inner.0);
            command.args(inner.1);
            command
        }
    }
}

fn inner_program_args(
    request: &ExecRequest,
    spec: &LocalCommandSpec,
    policy: &LocalBackendPolicy,
    cwd: Option<&std::path::Path>,
) -> (String, Vec<String>) {
    if let Some(profile) = policy.seatbelt_profile() {
        // The seatbelt profile text is a runtime value; embed it through the
        // launcher argv (same shape the previous early-return produced).
        let mut args = vec!["-p".to_string(), profile.to_profile_text()];
        args.push(spec.program.clone());
        args.extend(spec.args.iter().cloned());
        return ("sandbox-exec".to_string(), args);
    }

    if let Some(cwd) = cwd {
        if let Some(args) = policy.linux_dynsandbox_args(cwd, request.extra.as_ref()) {
            let mut sandbox_args = args;
            sandbox_args.push("--".to_string());
            sandbox_args.push(spec.program.clone());
            sandbox_args.extend(spec.args.iter().cloned());
            tracing::info!(
                "dyn-sandbox command built: dyn-sandbox {}",
                sandbox_args.join(" ")
            );
            return ("dyn-sandbox".to_string(), sandbox_args);
        }
        if let Some(args) = policy.bubblewrap_args(cwd) {
            let mut sandbox_args = args;
            sandbox_args.push(spec.program.clone());
            sandbox_args.extend(spec.args.iter().cloned());
            return ("bwrap".to_string(), sandbox_args);
        }
    }

    (spec.program.clone(), spec.args.clone())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../../../../../../tests/unit/operation_backend/backends/local/exec/command_spec_bubblewrap_test.rs"]
mod linux_bubblewrap_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "../../../../../../tests/unit/operation_backend/backends/local/exec/command_spec_dynsandbox_test.rs"]
mod linux_dynsandbox_tests;

#[cfg(test)]
#[path = "../../../../../../tests/unit/operation_backend/backends/local/exec/command_spec_spawn_wrapper_test.rs"]
mod spawn_wrapper_tests;
