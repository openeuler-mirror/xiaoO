use crate::backends::conch::agent;
use crate::backends::conch::backend::shell_quote;
use crate::backends::conch::backend::{ConchBackendState, ConchExecOutput, ConchStartProcess};
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
            env: HashMap::new(),
            cwd: request.cwd.map(|path| path.0),
            content: Some(script),
        }
    }

    fn timeout_wrapped_exec_request(
        &self,
        request: ExecRequest,
        timeout_ms: u64,
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
            env: HashMap::new(),
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
                Some(timeout_ms) => self.timeout_wrapped_shell_request(shell, request, timeout_ms),
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
                Some(timeout_ms) => self.timeout_wrapped_exec_request(request, timeout_ms),
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
