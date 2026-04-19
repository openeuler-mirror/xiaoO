use crate::backends::conch::agent;
use crate::backends::conch::backend::{
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
}

#[async_trait]
impl OperationExec for ConchExec {
    async fn exec(&self, request: ExecRequest) -> Result<ExecResult, OperationError> {
        self.state.ensure_active()?;
        let output = if let Some(shell) = request.shell {
            if !request.args.is_empty() {
                return Err(OperationError::Unsupported {
                    message: "shell execution does not support args".to_string(),
                });
            }
            agent::start_process(
                &self.state,
                ConchStartProcess {
                    cmd: shell,
                    args: Vec::new(),
                    env: HashMap::new(),
                    cwd: request.cwd.map(|path| path.0),
                    content: Some(request.command),
                },
            )
            .await?
        } else {
            agent::start_process(
                &self.state,
                ConchStartProcess {
                    cmd: request.command,
                    args: request.args,
                    env: HashMap::new(),
                    cwd: request.cwd.map(|path| path.0),
                    content: None,
                },
            )
            .await?
        };

        Ok(ExecResult {
            stdout: output.stdout,
            stderr: output.stderr,
            exit_code: output.exit_code,
            timed_out: output.timed_out,
        })
    }
}
