use std::path::PathBuf;
use std::sync::Arc;

use agent_contracts::backend::capability::filesystem::ReadBytesRequest;
use agent_contracts::backend::capability::path::{ResolveBase, ResolvePathRequest};
use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

use crate::r#impl::ToolRuntimeServices;

use super::super::validation::backend as validation;
use super::input::{LspAction, LspInput};
use super::output::LspOutput;
use super::spec::LspToolSpec;

pub struct LspExecutor {
    spec: Arc<LspToolSpec>,
    services: ToolRuntimeServices,
}

impl LspExecutor {
    pub fn new(spec: Arc<LspToolSpec>, services: ToolRuntimeServices) -> Self {
        Self { spec, services }
    }
}

#[async_trait]
impl ToolExecutor for LspExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: LspInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("invalid input: {e}"),
            }
        })?;

        let validation_result = validation::validate_input(&input);
        if !validation_result.result {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: validation_result.message.unwrap_or_default(),
                },
            });
        }

        let Some(registry) = &self.services.lsp_registry else {
            return Ok(error_output(
                "LSP service is not enabled. Set lsp.enabled = true in daemon config.",
            ));
        };

        // ── Operation backend: required ────────────────────────────────────────
        let Some(backend) = runtime.operation_backend() else {
            return Ok(error_output(
                "lsp requires operation backend access, but none is configured",
            ));
        };

        if !backend.capabilities().supports_lsp {
            return Ok(error_output(&format!(
                "lsp tool is not supported by backend '{}'",
                backend.backend_id(),
            )));
        }

        let Some(lsp) = registry.get_or_create(Arc::clone(&backend)) else {
            return Ok(error_output(
                "failed to initialize LSP service for local backend",
            ));
        };

        // ── Path resolution via operation backend ──────────────────────────────
        let resolved_path = backend
            .paths()
            .resolve_path(ResolvePathRequest {
                raw_path: input.file_path.clone(),
                base: ResolveBase::WorkspaceRoot,
            })
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("failed to resolve path: {e}"),
            })?;
        let resolved = resolved_path.0.clone();
        let file = PathBuf::from(&resolved);

        // ── Explicit content sync ──────────────────────────────────────────────
        // For local backend, LspService also reads content from the host FS
        // inside prepare_file(). This explicit sync ensures the LSP server sees
        // the current on-disk state and establishes the interface that future
        // remote backends (conch) will use as their primary content delivery path.
        let file_content = backend
            .files()
            .read_bytes(ReadBytesRequest {
                path: resolved_path,
            })
            .await
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .unwrap_or_default();
        lsp.open_file(&file, file_content).await;

        let output: LspOutput = match input.action {
            LspAction::Diagnostics => {
                let items = lsp.diagnostics(&file).await.map_err(exec_err)?;
                let has_errors = items.iter().any(|d| d.severity == "error");
                let count = items.len();
                LspOutput::Diagnostics {
                    file: resolved,
                    has_errors,
                    count,
                    items,
                }
            }

            LspAction::Hover => {
                let (line, col) = require_position(&input)?;
                let content = lsp.hover(&file, line, col).await.map_err(exec_err)?;
                LspOutput::Hover {
                    file: resolved,
                    line,
                    column: col,
                    content,
                }
            }

            LspAction::Definition => {
                let (line, col) = require_position(&input)?;
                let locations = lsp.definition(&file, line, col).await.map_err(exec_err)?;
                let count = locations.len();
                LspOutput::Definition { locations, count }
            }

            LspAction::References => {
                let (line, col) = require_position(&input)?;
                let locations = lsp
                    .references(&file, line, col, input.include_declaration)
                    .await
                    .map_err(exec_err)?;
                let count = locations.len();
                LspOutput::References { locations, count }
            }

            LspAction::Symbols => {
                let symbols = lsp
                    .symbols(&file, input.query.as_deref())
                    .await
                    .map_err(exec_err)?;
                let count = symbols.len();
                LspOutput::Symbols { symbols, count }
            }

            LspAction::Implementation => {
                let (line, col) = require_position(&input)?;
                let locations = lsp
                    .implementation(&file, line, col)
                    .await
                    .map_err(exec_err)?;
                let count = locations.len();
                LspOutput::Implementation { locations, count }
            }

            LspAction::PrepareCallHierarchy => {
                let (line, col) = require_position(&input)?;
                let items = lsp
                    .prepare_call_hierarchy(&file, line, col)
                    .await
                    .map_err(exec_err)?;
                let count = items.len();
                LspOutput::PrepareCallHierarchy { items, count }
            }

            LspAction::IncomingCalls => {
                let (line, col) = require_position(&input)?;
                let calls = lsp
                    .incoming_calls(&file, line, col)
                    .await
                    .map_err(exec_err)?;
                let count = calls.len();
                LspOutput::IncomingCalls { calls, count }
            }

            LspAction::OutgoingCalls => {
                let (line, col) = require_position(&input)?;
                let calls = lsp
                    .outgoing_calls(&file, line, col)
                    .await
                    .map_err(exec_err)?;
                let count = calls.len();
                LspOutput::OutgoingCalls { calls, count }
            }
        };

        let json = serde_json::to_string(&output).map_err(exec_err)?;
        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success { output: json },
        })
    }
}

fn require_position(input: &LspInput) -> Result<(u32, u32), ToolExecutionError> {
    match (input.line, input.column) {
        (Some(l), Some(c)) => Ok((l, c)),
        _ => Err(ToolExecutionError::ExecutionFailed {
            message: format!(
                "action '{:?}' requires both 'line' and 'column'",
                input.action
            ),
        }),
    }
}

fn exec_err<E: std::fmt::Display>(e: E) -> ToolExecutionError {
    ToolExecutionError::ExecutionFailed {
        message: e.to_string(),
    }
}

fn error_output(msg: &str) -> ToolExecutorOutput {
    ToolExecutorOutput::Completed {
        raw_outcome: RawToolOutcome::Error {
            message: msg.to_string(),
        },
    }
}
