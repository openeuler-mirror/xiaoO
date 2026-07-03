#[cfg(unix)]
#[allow(unused_imports)]
use std::os::unix::process::CommandExt;

use crate::backends::local::backend::LocalBackendState;
use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    OperationError,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio::time::timeout;

pub(crate) struct LocalExec {
    _state: Arc<LocalBackendState>,
}

impl LocalExec {
    pub(crate) fn new(state: Arc<LocalBackendState>) -> Self {
        Self { _state: state }
    }
}

#[async_trait]
impl OperationExec for LocalExec {
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        let command_spec = build_command_spec(self._state.default_shell.as_deref(), &request)?;
        let command_cwd = if let Some(cwd) = request.cwd.as_ref() {
            let cwd = self._state.backend_path_to_host(cwd)?;
            self._state.policy.check_exec_cwd(cwd.as_path())?;
            self._state.ensure_directory(cwd.as_path())?;
            Some(cwd)
        } else if self._state.policy.requires_exec_cwd() {
            let cwd = self._state.workspace_root_host.clone();
            self._state.policy.check_exec_cwd(cwd.as_path())?;
            self._state.ensure_directory(cwd.as_path())?;
            Some(cwd)
        } else {
            None
        };

        let mut command =
            command_from_spec(command_spec, &self._state.policy, command_cwd.as_deref());

        if let Some(env_vars) = &request.env {
            for (k, v) in env_vars {
                command.env(k, v);
            }
        }

        if let Some(cwd) = command_cwd {
            command.current_dir(cwd);
        }

        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        {
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;

        #[cfg(unix)]
        let pgid = child.id().unwrap_or(0) as i32;
        #[cfg(unix)]
        if pgid > 0 {
            crate::process_group::register_pgid(pgid);
        }

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "failed to capture stdout".to_string(),
            })?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| OperationError::ExecutionFailed {
                message: "failed to capture stderr".to_string(),
            })?;

        let stdout_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let stderr_task = tokio::spawn(async move {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).await.map(|_| bytes)
        });

        let (exit_code, timed_out) = if let Some(timeout_ms) = request.timeout_ms {
            match timeout(Duration::from_millis(timeout_ms), child.wait()).await {
                Ok(status) => {
                    let status = status.map_err(|error| OperationError::ExecutionFailed {
                        message: error.to_string(),
                    })?;
                    #[cfg(unix)]
                    crate::process_group::unregister_pgid(pgid);
                    (status.code(), false)
                }
                Err(_) => {
                    #[cfg(unix)]
                    {
                        crate::process_group::send_sigterm_to_group(pgid);
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        crate::process_group::send_sigkill_to_group(pgid);
                    }
                    #[cfg(not(unix))]
                    {
                        let _ = child.kill().await;
                    }
                    let _ = child.wait().await;
                    (None, true)
                }
            }
        } else {
            let status = child
                .wait()
                .await
                .map_err(|error| OperationError::ExecutionFailed {
                    message: error.to_string(),
                })?;
            #[cfg(unix)]
            crate::process_group::unregister_pgid(pgid);
            (status.code(), false)
        };

        let stdout = stdout_task
            .await
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;
        let stderr = stderr_task
            .await
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?
            .map_err(|error| OperationError::ExecutionFailed {
                message: error.to_string(),
            })?;

        Ok(ExecResult {
            stdout,
            stderr,
            exit_code,
            timed_out,
        })
    }
}

struct LocalCommandSpec {
    program: String,
    args: Vec<String>,
}

fn build_command_spec(
    default_shell: Option<&str>,
    request: &ExecRequest,
) -> Result<LocalCommandSpec, OperationError> {
    if request.command.trim().is_empty() {
        return Err(OperationError::ExecutionFailed {
            message: "command cannot be empty".to_string(),
        });
    }

    if let Some(shell) = request.shell.as_deref().or(default_shell) {
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

fn command_from_spec(
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
mod linux_bubblewrap_tests {
    use super::*;
    use crate::backends::local::factory::local_backend_with_isolation;
    use agent_contracts::backend::{
        BackendPath, SandboxPermissionCapability, SandboxPermissionGrantRequest,
        SandboxPermissionScope, SandboxPolicyDenial,
    };
    use serde_json::json;
    use std::path::{Path, PathBuf};

    #[test]
    fn bubblewrap_exec_enforces_filesystem_policy() {
        if !has_bwrap() {
            return;
        }
        // Serialize against the process-group unit tests: `exec` registers real
        // child pgids in the shared global registry, and `kill_all_process_groups`
        // would otherwise clear it (and signal our children) mid-test.
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = test_root("fs");
        let workspace = root.join("workspace");
        let writable = workspace.join("tmp");
        let outside = root.join("outside");
        std::fs::create_dir_all(writable.as_path()).unwrap();
        std::fs::create_dir_all(outside.as_path()).unwrap();
        std::fs::write(workspace.join("readable.txt"), b"visible").unwrap();
        std::fs::write(outside.join("secret.txt"), b"secret").unwrap();

        let backend = bubblewrap_backend(workspace.clone(), writable.clone(), false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let read = runtime.block_on(exec_bash(backend.as_ref(), &workspace, "cat readable.txt"));
        assert_eq!(read.exit_code, Some(0));
        assert_eq!(String::from_utf8_lossy(read.stdout.as_slice()), "visible");

        let write = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            "printf ok > tmp/out.txt",
        ));
        assert_eq!(write.exit_code, Some(0));
        assert_eq!(
            std::fs::read_to_string(writable.join("out.txt")).unwrap(),
            "ok"
        );

        let denied_write = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            "printf no > blocked.txt",
        ));
        assert_ne!(denied_write.exit_code, Some(0));
        assert!(!workspace.join("blocked.txt").exists());

        let outside_path = outside.join("secret.txt");
        let denied_read = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            format!("cat {}", shell_quote(outside_path.as_path())).as_str(),
        ));
        assert_ne!(denied_read.exit_code, Some(0));
        assert!(!String::from_utf8_lossy(denied_read.stdout.as_slice()).contains("secret"));

        backend
            .permission_control()
            .unwrap()
            .grant(SandboxPermissionGrantRequest {
                denial: SandboxPolicyDenial {
                    backend_id: backend.backend_id().to_string(),
                    isolation: "linux_bubblewrap".to_string(),
                    operation: "test".to_string(),
                    capability: SandboxPermissionCapability::Read,
                    path: outside_path.display().to_string(),
                },
                scope: SandboxPermissionScope::Session,
            })
            .unwrap();

        let granted_read = runtime.block_on(exec_bash(
            backend.as_ref(),
            &workspace,
            format!("cat {}", shell_quote(outside_path.as_path())).as_str(),
        ));
        assert_eq!(granted_read.exit_code, Some(0));
        assert_eq!(
            String::from_utf8_lossy(granted_read.stdout.as_slice()),
            "secret"
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    #[test]
    fn bubblewrap_exec_can_unshare_network() {
        if !has_bwrap() {
            return;
        }
        let _guard = crate::process_group::process_group_test_lock()
            .lock()
            .unwrap();
        let root = test_root("net");
        let workspace = root.join("workspace");
        let writable = workspace.join("tmp");
        std::fs::create_dir_all(writable.as_path()).unwrap();

        let backend = bubblewrap_backend(workspace.clone(), writable, false);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime.block_on(exec_bash(backend.as_ref(), &workspace, "cat /proc/net/dev"));

        assert_eq!(result.exit_code, Some(0));
        let interfaces = String::from_utf8_lossy(result.stdout.as_slice())
            .lines()
            .filter_map(|line| {
                line.split_once(':')
                    .map(|(name, _)| name.trim().to_string())
            })
            .collect::<Vec<_>>();
        assert!(
            interfaces.iter().all(|name| name == "lo"),
            "unexpected interfaces in unshared network namespace: {interfaces:?}"
        );

        let _ = std::fs::remove_dir_all(root.as_path());
    }

    fn bubblewrap_backend(
        workspace: PathBuf,
        writable: PathBuf,
        allow_network: bool,
    ) -> std::sync::Arc<dyn agent_contracts::backend::OperationBackend> {
        local_backend_with_isolation(
            workspace.clone(),
            None,
            Some(writable.clone()),
            None,
            Some(json!({
                "kind": "linux_bubblewrap",
                "allow_network": allow_network,
                "readable_roots": [workspace.to_string_lossy().to_string()],
                "writable_roots": [writable.to_string_lossy().to_string()]
            })),
        )
        .unwrap()
    }

    async fn exec_bash(
        backend: &dyn agent_contracts::backend::OperationBackend,
        cwd: &Path,
        command: &str,
    ) -> ExecResult {
        backend
            .exec()
            .exec(ExecRequest {
                command: command.to_string(),
                args: vec![],
                shell: Some("bash".to_string()),
                cwd: Some(BackendPath(cwd.to_string_lossy().into_owned())),
                timeout_ms: Some(5_000),
                env: None,
            })
            .await
            .unwrap()
    }

    fn has_bwrap() -> bool {
        std::env::var_os("PATH")
            .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join("bwrap").is_file()))
            .unwrap_or(false)
    }

    fn test_root(name: &str) -> PathBuf {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let root = std::env::temp_dir().join(format!(
            "xiaoo-bubblewrap-{name}-{}-{millis}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(root.as_path());
        std::fs::create_dir_all(root.join("workspace")).unwrap();
        root
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }
}
