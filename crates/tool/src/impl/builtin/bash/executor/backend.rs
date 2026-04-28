use std::sync::Arc;

use async_trait::async_trait;

use agent_contracts::backend::capability::exec::ExecRequest;
use agent_contracts::backend::BackendPath;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::super::validation::backend as validation;
use super::constants::{default_timeout_ms, MAX_OUTPUT_BYTES_PER_STREAM};
use super::input::BashInput;
use super::output::BashOutput;
use super::spec::BashToolSpec;

pub struct BashExecutor {
    spec: Arc<BashToolSpec>,
}

impl BashExecutor {
    pub fn new(spec: Arc<BashToolSpec>) -> Self {
        Self { spec }
    }

    async fn resolve_and_stat_cwd(
        cwd: Option<&str>,
        backend: &dyn agent_contracts::backend::OperationBackend,
    ) -> Result<Option<(BackendPath, agent_contracts::backend::PathStat)>, String> {
        let Some(cwd) = cwd else {
            return Ok(None);
        };

        let cwd_str = cwd.trim();

        let base = agent_contracts::backend::capability::path::ResolveBase::WorkspaceRoot;
        let resolved = backend
            .paths()
            .resolve_path(
                agent_contracts::backend::capability::path::ResolvePathRequest {
                    raw_path: cwd_str.to_string(),
                    base,
                },
            )
            .await
            .map_err(|e| format!("Failed to resolve cwd path: {}", e))?;

        let stat = backend
            .files()
            .stat(&resolved)
            .await
            .map_err(|e| format!("Failed to stat cwd path: {}", e))?;

        Ok(Some((resolved, stat)))
    }

    fn format_output(
        result: &agent_contracts::backend::capability::exec::ExecResult,
    ) -> BashOutput {
        let stdout_truncated = result.stdout.len() > MAX_OUTPUT_BYTES_PER_STREAM;
        let stderr_truncated = result.stderr.len() > MAX_OUTPUT_BYTES_PER_STREAM;
        let stdout_bytes = if stdout_truncated {
            &result.stdout[..MAX_OUTPUT_BYTES_PER_STREAM]
        } else {
            result.stdout.as_slice()
        };
        let stderr_bytes = if stderr_truncated {
            &result.stderr[..MAX_OUTPUT_BYTES_PER_STREAM]
        } else {
            result.stderr.as_slice()
        };

        BashOutput {
            stdout: String::from_utf8_lossy(stdout_bytes).replace("\r\n", "\n"),
            stdout_truncated,
            stderr: String::from_utf8_lossy(stderr_bytes).replace("\r\n", "\n"),
            stderr_truncated,
            exit_code: result.exit_code,
            interrupted: result.timed_out,
        }
    }
}

impl Default for BashExecutor {
    fn default() -> Self {
        Self::new(Arc::new(BashToolSpec::new()))
    }
}

#[async_trait]
impl ToolExecutor for BashExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: BashInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "bash requires operation backend access, but none is configured"
                        .to_string(),
                },
            });
        }
        let backend = backend.unwrap();

        let validation_result = validation::validate_command(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let validation_result = validation::validate_timeout(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: format!("[error_code={}] {}", error_code, error_message),
                },
            });
        }

        let cwd = Self::resolve_and_stat_cwd(input.cwd.as_deref(), &*backend)
            .await
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        let cwd_path = if let Some((resolved, stat)) = cwd {
            let cwd_str = input.cwd.as_deref().unwrap_or_default();
            let validation_result = validation::validate_cwd_backend(cwd_str, &stat);
            if !validation_result.result {
                let error_message = validation_result
                    .message
                    .unwrap_or_else(|| "Validation failed".to_string());
                let error_code = validation_result.error_code.unwrap_or(0);
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: format!("[error_code={}] {}", error_code, error_message),
                    },
                });
            }
            Some(resolved)
        } else {
            None
        };

        let request = ExecRequest {
            command: input.command.clone(),
            args: vec![],
            shell: Some("bash".to_string()),
            cwd: cwd_path,
            timeout_ms: Some(input.timeout.unwrap_or_else(default_timeout_ms)),
            env: None,
        };

        let result = backend.exec().exec(request).await.map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Backend exec failed: {}", e),
            }
        })?;

        let output = Self::format_output(&result);

        let serialized =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output: serialized },
        })
    }
}
