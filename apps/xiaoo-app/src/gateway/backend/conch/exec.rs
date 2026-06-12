use crate::gateway::backend::conch::agent;
use crate::gateway::backend::conch::backend::shell_quote;
use crate::gateway::backend::conch::backend::{
    ConchBackendState, ConchExecOutput, ConchStartProcess,
};
use agent_contracts::backend::{
    capability::{exec::ExecRequest, exec::ExecResult, OperationExec},
    OperationError,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;

pub(crate) struct ConchExec {
    state: Arc<ConchBackendState>,
}

impl ConchExec {
    pub(crate) fn new(state: Arc<ConchBackendState>) -> Self {
        Self { state }
    }

    pub(crate) fn state(&self) -> &Arc<ConchBackendState> {
        &self.state
    }

    pub(crate) async fn run_shell_script(
        &self,
        script: &str,
        cwd: Option<&str>,
    ) -> Result<ConchExecOutput, OperationError> {
        let shell = self
            .state
            .default_shell
            .as_deref()
            .unwrap_or("/bin/sh")
            .to_string();
        agent::start_process(
            &self.state,
            ConchStartProcess {
                cmd: shell,
                args: Vec::new(),
                env: HashMap::new(),
                cwd: cwd.map(str::to_string),
                content: Some(script.to_string()),
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

    fn timeout_wrapped_shell_request(
        &self,
        shell: String,
        request: ExecRequest,
        timeout_ms: u64,
        env: HashMap<String, String>,
    ) -> ConchStartProcess {
        let heredoc = format!("__XIAOO_TIMEOUT_SCRIPT_{}__", uuid::Uuid::new_v4().simple());
        let script = format!(
            "if ! command -v timeout >/dev/null 2>&1; then\n  printf '%s\\n' 'conch backend requires `timeout` command for timeout-managed shell execution' >&2\n  exit 127\nfi\nexec timeout --signal=TERM {} {} <<'{}'\n{}\n{}\n",
            Self::timeout_arg(timeout_ms),
            shell_quote(shell.as_str()),
            heredoc,
            request.command,
            heredoc,
        );

        ConchStartProcess {
            cmd: self
                .state
                .default_shell
                .as_deref()
                .unwrap_or("/bin/sh")
                .to_string(),
            args: Vec::new(),
            env,
            cwd: request.cwd.map(|path| path.0),
            content: Some(script),
        }
    }

    fn timeout_wrapped_exec_request(
        &self,
        request: ExecRequest,
        timeout_ms: u64,
        env: HashMap<String, String>,
    ) -> ConchStartProcess {
        let mut args = vec![
            "--signal=TERM".to_string(),
            Self::timeout_arg(timeout_ms),
            request.command,
        ];
        args.extend(request.args);

        ConchStartProcess {
            cmd: "timeout".to_string(),
            args,
            env,
            cwd: request.cwd.map(|path| path.0),
            content: None,
        }
    }
}

#[async_trait]
impl OperationExec for ConchExec {
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        self.state.ensure_active()?;
        let timeout_ms = request.timeout_ms;
        let extra_env: HashMap<String, String> = request
            .env
            .as_ref()
            .map(|pairs| pairs.iter().cloned().collect())
            .unwrap_or_default();
        let output = if let Some(shell) = request.shell.clone() {
            if !request.args.is_empty() {
                return Err(OperationError::Unsupported {
                    message: "shell execution does not support args".to_string(),
                });
            }
            let process = match timeout_ms {
                Some(timeout_ms) => {
                    self.timeout_wrapped_shell_request(shell, request, timeout_ms, extra_env)
                }
                None => ConchStartProcess {
                    cmd: shell,
                    args: Vec::new(),
                    env: extra_env,
                    cwd: request.cwd.map(|path| path.0),
                    content: Some(request.command),
                },
            };
            agent::start_process(&self.state, process).await?
        } else {
            let process = match timeout_ms {
                Some(timeout_ms) => {
                    self.timeout_wrapped_exec_request(request, timeout_ms, extra_env)
                }
                None => ConchStartProcess {
                    cmd: request.command,
                    args: request.args,
                    env: extra_env,
                    cwd: request.cwd.map(|path| path.0),
                    content: None,
                },
            };
            agent::start_process(&self.state, process).await?
        };

        let timed_out = timeout_ms.is_some() && output.exit_code == Some(124);

        Ok(ExecResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: output.timed_out || timed_out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::backend::conch::backend::{
        ConchControlPlane, ConchControlTransport, ConchLifecycle, ConchSandboxHandle,
    };
    use agent_contracts::backend::BackendPath;
    use std::sync::Mutex;

    fn exec() -> ConchExec {
        ConchExec::new(Arc::new(ConchBackendState {
            backend_id: "conch:test".to_string(),
            workspace_root: BackendPath("/workspace".to_string()),
            home_dir: None,
            temp_root: BackendPath("/tmp".to_string()),
            default_shell: None,
            control_plane: ConchControlPlane {
                transport: ConchControlTransport::ApiUrl("http://conch".to_string()),
                namespace: String::new(),
            },
            sandbox: ConchSandboxHandle {
                sandbox_id: "sandbox".to_string(),
                ip: "127.0.0.1".to_string(),
                agent_port: 4064,
            },
            lifecycle: Mutex::new(ConchLifecycle::Active),
        }))
    }

    fn request() -> ExecRequest {
        ExecRequest {
            command: "printenv XIAOO_ENV".to_string(),
            args: Vec::new(),
            shell: Some("/bin/sh".to_string()),
            cwd: Some(BackendPath("/workspace".to_string())),
            timeout_ms: Some(1000),
            env: Some(vec![("XIAOO_ENV".to_string(), "kept".to_string())]),
        }
    }

    #[test]
    fn timeout_wrapped_shell_request_preserves_env() {
        let exec = exec();
        let mut env = HashMap::new();
        env.insert("XIAOO_ENV".to_string(), "kept".to_string());

        let process =
            exec.timeout_wrapped_shell_request("/bin/sh".to_string(), request(), 1000, env);

        assert_eq!(process.env.get("XIAOO_ENV"), Some(&"kept".to_string()));
    }

    #[test]
    fn timeout_wrapped_exec_request_preserves_env() {
        let exec = exec();
        let mut env = HashMap::new();
        env.insert("XIAOO_ENV".to_string(), "kept".to_string());
        let mut request = request();
        request.shell = None;

        let process = exec.timeout_wrapped_exec_request(request, 1000, env);

        assert_eq!(process.env.get("XIAOO_ENV"), Some(&"kept".to_string()));
    }
}
