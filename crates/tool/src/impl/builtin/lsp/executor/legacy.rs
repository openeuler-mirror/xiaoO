#![allow(unused)]
use std::path::PathBuf;
use std::sync::Arc;

use agent_contracts::runtime::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};
use async_trait::async_trait;

use crate::r#impl::path_resolver::{expand_path_from_base, runtime_workspace_root};
use crate::r#impl::ToolRuntimeServices;

use super::super::validation::legacy as validation;
use super::input::{LspAction, LspInput};
use super::output::LspOutput;
use super::spec::LspToolSpec;

pub struct LspLegacyExecutor {
    spec: Arc<LspToolSpec>,
    services: ToolRuntimeServices,
}

impl LspLegacyExecutor {
    pub fn new(spec: Arc<LspToolSpec>, services: ToolRuntimeServices) -> Self {
        Self { spec, services }
    }
}

#[async_trait]
impl ToolExecutor for LspLegacyExecutor {
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
        let Some(lsp) = registry.get_or_create(operation_backend::local_lsp_backend()) else {
            return Ok(error_output(
                "LSP service is not enabled. Set lsp.enabled = true in daemon config.",
            ));
        };

        let workspace_root = runtime_workspace_root(runtime);
        let resolved = expand_path_from_base(&input.file_path, workspace_root);
        let file = PathBuf::from(&resolved);

        lsp.touch_file(&file).await;

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
