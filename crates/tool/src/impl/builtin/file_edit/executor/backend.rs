use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use agent_contracts::backend::capability::filesystem::{
    ReadBytesRequest, WriteBytesRequest, WriteMode,
};
use agent_contracts::backend::capability::path::ResolveBase;
use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::executor::ToolExecutor;
use agent_contracts::tool::spec::ToolSpecView;
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError, ToolExecutorOutput};

use super::super::validation::backend as validation;
use super::input::FileEditInput;
use super::output::FileEditOutput;
use super::spec::FileEditToolSpec;
use super::utils::{
    apply_edit_to_file, find_actual_string, get_patch_for_edit, preserve_quote_style,
};
use crate::r#impl::builtin::file_read::dedup::{system_time_to_timestamp, DedupStateStore};
use crate::r#impl::fs_timeout::{timed, DEFAULT_FS_TIMEOUT_MS};
use crate::r#impl::lsp_hooks::{fetch_diagnostics, spawn_touch_file};
use crate::r#impl::ToolRuntimeServices;

const LSP_DIAG_TIMEOUT_SECS: u64 = 15;

pub struct FileEditExecutor {
    spec: Arc<FileEditToolSpec>,
    dedup_store: Arc<Mutex<DedupStateStore>>,
    services: ToolRuntimeServices,
}

impl FileEditExecutor {
    pub fn new(spec: Arc<FileEditToolSpec>, services: ToolRuntimeServices) -> Self {
        Self::new_with_state(spec, services, Arc::new(Mutex::new(DedupStateStore::new())))
    }

    pub(crate) fn new_with_state(
        spec: Arc<FileEditToolSpec>,
        services: ToolRuntimeServices,
        dedup_store: Arc<Mutex<DedupStateStore>>,
    ) -> Self {
        Self {
            spec,
            dedup_store,
            services,
        }
    }

    async fn get_dedup_store(&self) -> tokio::sync::MutexGuard<'_, DedupStateStore> {
        self.dedup_store.lock().await
    }
}

impl Default for FileEditExecutor {
    fn default() -> Self {
        Self::new(
            Arc::new(FileEditToolSpec::new()),
            ToolRuntimeServices::default(),
        )
    }
}

#[async_trait]
impl ToolExecutor for FileEditExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        runtime: &dyn RuntimeView,
    ) -> Result<ToolExecutorOutput, ToolExecutionError> {
        let input: FileEditInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let backend = runtime.operation_backend();
        if backend.is_none() {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message: "file_edit requires operation backend access, but none is configured"
                        .to_string(),
                },
            });
        }
        let backend = backend.unwrap();
        let lsp = self
            .services
            .lsp_registry
            .as_ref()
            .and_then(|reg| reg.get_or_create(Arc::clone(&backend)));

        let resolved = timed(
            "file_edit resolve_path",
            DEFAULT_FS_TIMEOUT_MS,
            backend.paths().resolve_path(
                agent_contracts::backend::capability::path::ResolvePathRequest {
                    raw_path: input.file_path.trim().to_string(),
                    base: ResolveBase::WorkspaceRoot,
                },
            ),
        )
        .await
        .map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to resolve path: {}", e),
        })?;

        let resolved_str = resolved.to_string();

        let stat = timed(
            "file_edit stat",
            DEFAULT_FS_TIMEOUT_MS,
            backend.files().stat(&resolved),
        )
        .await
        .map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to stat file: {}", e),
        })?;

        let file_content = if stat.exists {
            let bytes = timed(
                "file_edit read_bytes",
                DEFAULT_FS_TIMEOUT_MS,
                backend.files().read_bytes(ReadBytesRequest {
                    path: resolved.clone(),
                }),
            )
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to read file: {}", e),
            })?;
            Some(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            None
        };

        let mtime = system_time_to_timestamp(stat.modified_at);

        let validation_result = {
            let dedup_store = self.get_dedup_store().await;
            validation::validate_input_backend(
                &input,
                file_content.as_deref(),
                &dedup_store,
                &resolved_str,
                &stat,
                mtime,
            )
        };
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

        if input.old_string.is_empty() {
            let capabilities = backend.capabilities();
            let write_mode = if capabilities.supports_atomic_write {
                WriteMode::AtomicOverwrite
            } else {
                return Ok(ToolExecutorOutput::Completed {
                    raw_outcome: RawToolOutcome::Error {
                        message: "file_edit requires atomic write support, but backend does not support it"
                            .to_string(),
                    },
                });
            };

            timed(
                "file_edit write_bytes",
                DEFAULT_FS_TIMEOUT_MS,
                backend.files().write_bytes(WriteBytesRequest {
                    path: resolved,
                    content: input.new_string.as_bytes().to_vec(),
                    mode: write_mode,
                }),
            )
            .await
            .map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to write file: {}", e),
            })?;

            if let Some(ref lsp) = lsp {
                spawn_touch_file(lsp, std::path::Path::new(&resolved_str));
            }

            let lsp_diagnostics = if let Some(ref lsp) = lsp {
                fetch_diagnostics(
                    lsp,
                    std::path::Path::new(&resolved_str),
                    LSP_DIAG_TIMEOUT_SECS,
                )
                .await
            } else {
                None
            };

            let output = FileEditOutput {
                file_path: input.file_path.clone(),
                new_lines: input.new_string.lines().count() as u32,
                structured_patch: Vec::new(),
                user_modified: false,
                replace_all: false,
                git_diff: None,
                lsp_diagnostics,
            };

            let json_output = serde_json::to_string(&output).map_err(|e| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("Failed to serialize output: {}", e),
                }
            })?;
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success {
                    output: json_output,
                },
            });
        }

        let content = file_content.ok_or_else(|| ToolExecutionError::ExecutionFailed {
            message: format!("File not found: {}", resolved_str),
        })?;

        let actual_old_string =
            find_actual_string(&content, &input.old_string).ok_or_else(|| {
                ToolExecutionError::ExecutionFailed {
                    message: format!("old_string not found in file: {}", input.old_string),
                }
            })?;

        let styled_new_string = preserve_quote_style(&actual_old_string, &input.new_string);

        let updated_content = apply_edit_to_file(
            &content,
            &actual_old_string,
            &styled_new_string,
            input.replace_all,
        )
        .ok_or_else(|| ToolExecutionError::ExecutionFailed {
            message: "Failed to apply edit: old_string not found in file".to_string(),
        })?;

        let (structured_patch, _updated_file) =
            get_patch_for_edit(&actual_old_string, &styled_new_string);

        let capabilities = backend.capabilities();
        let write_mode = if capabilities.supports_atomic_write {
            WriteMode::AtomicOverwrite
        } else {
            return Ok(ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error {
                    message:
                        "file_edit requires atomic write support, but backend does not support it"
                            .to_string(),
                },
            });
        };

        timed(
            "file_edit write_bytes",
            DEFAULT_FS_TIMEOUT_MS,
            backend.files().write_bytes(WriteBytesRequest {
                path: resolved,
                content: updated_content.as_bytes().to_vec(),
                mode: write_mode,
            }),
        )
        .await
        .map_err(|e| ToolExecutionError::ExecutionFailed {
            message: format!("Failed to write file: {}", e),
        })?;

        if let Some(ref lsp) = lsp {
            spawn_touch_file(lsp, std::path::Path::new(&resolved_str));
        }

        let lsp_diagnostics = if let Some(ref lsp) = lsp {
            fetch_diagnostics(
                lsp,
                std::path::Path::new(&resolved_str),
                LSP_DIAG_TIMEOUT_SECS,
            )
            .await
        } else {
            None
        };

        let output = FileEditOutput {
            file_path: input.file_path.clone(),
            new_lines: updated_content.lines().count() as u32,
            structured_patch,
            user_modified: false,
            replace_all: input.replace_all,
            git_diff: None,
            lsp_diagnostics,
        };

        let json_output =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(ToolExecutorOutput::Completed {
            raw_outcome: RawToolOutcome::Success {
                output: json_output,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use agent_contracts::backend::OperationBackend;
    use agent_contracts::{
        AgentContext, HookerRegistry, InteractionHandle, RuntimeView, ToolEventSink, ToolSource,
        ToolStateStore, TraceRecorder,
    };
    use agent_types::tool::{FinalToolCall, RawToolOutcome, ToolExecutorOutput};
    use serde_json::json;

    use crate::r#impl::builtin::BuiltinToolSource;
    use crate::r#impl::ToolRuntimeServices;

    struct TestRuntime(Arc<dyn OperationBackend>);

    // File tools only require an operation backend. Panic on any unexpected
    // runtime dependency so the fixture stays minimal and fails loudly.
    impl RuntimeView for TestRuntime {
        fn state_store(&self) -> &dyn ToolStateStore {
            panic!("not used in file tool tests")
        }

        fn tool_events(&self) -> &dyn ToolEventSink {
            panic!("not used in file tool tests")
        }

        fn trace_recorder(&self) -> &dyn TraceRecorder {
            panic!("not used in file tool tests")
        }

        fn agent_context(&self) -> &dyn AgentContext {
            panic!("not used in file tool tests")
        }

        fn interaction(&self) -> &dyn InteractionHandle {
            panic!("not used in file tool tests")
        }

        fn hookers(&self) -> &dyn HookerRegistry {
            panic!("not used in file tool tests")
        }

        fn operation_backend(&self) -> Option<Arc<dyn OperationBackend>> {
            Some(Arc::clone(&self.0))
        }
    }

    fn call(tool_name: &str, input: serde_json::Value) -> FinalToolCall {
        FinalToolCall {
            call_id: format!("{tool_name}-call"),
            tool_name: tool_name.to_string(),
            input,
        }
    }

    #[tokio::test]
    async fn rejects_edit_when_file_changed_after_read() {
        const ORIGINAL: &str = "fn main() {\n    let timeout = 30;\n}\n";
        const USER_EDIT: &str = "fn main() {\n    let timeout = 30;\n    enable_tls();\n}\n";

        let temp = tempfile::tempdir().expect("tempdir");
        let file_path = temp.path().join("sample.rs");
        std::fs::write(&file_path, ORIGINAL).expect("write initial file");
        let read_mtime = std::fs::metadata(&file_path)
            .and_then(|metadata| metadata.modified())
            .expect("initial mtime");

        let backend = operation_backend::local_backend(temp.path().to_path_buf(), None, None, None)
            .expect("local backend");
        let runtime = TestRuntime(backend);

        // Use the production source so both executors receive its private state.
        let tools = BuiltinToolSource::new(ToolRuntimeServices::default()).discover();
        let read_executor = tools
            .iter()
            .find(|tool| tool.spec.name().0 == "file_read")
            .expect("file_read tool");
        let edit_executor = tools
            .iter()
            .find(|tool| tool.spec.name().0 == "file_edit")
            .expect("file_edit tool");

        let read_output = read_executor
            .executor
            .invoke(
                &call("file_read", json!({ "file_path": "sample.rs" })),
                &runtime,
            )
            .await
            .expect("file_read execution");
        assert!(matches!(
            read_output,
            ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Success { .. }
            }
        ));

        // Simulate a user edit while keeping old_string present. This isolates
        // the stale-read guard from the ordinary "old_string not found" check.
        std::fs::write(&file_path, USER_EDIT).expect("modify file after read");

        // Advance mtime explicitly instead of sleeping for the filesystem clock.
        std::fs::File::options()
            .write(true)
            .open(&file_path)
            .and_then(|file| file.set_modified(read_mtime + Duration::from_secs(1)))
            .expect("advance modified time");

        let edit_output = edit_executor
            .executor
            .invoke(
                &call(
                    "file_edit",
                    json!({
                        "file_path": "sample.rs",
                        "old_string": "let timeout = 30;",
                        "new_string": "let timeout = 60;"
                    }),
                ),
                &runtime,
            )
            .await
            .expect("file_edit execution");

        match edit_output {
            ToolExecutorOutput::Completed {
                raw_outcome: RawToolOutcome::Error { message },
            } => assert!(
                message.starts_with("[error_code=7]"),
                "expected FILE_MODIFIED, got: {message}"
            ),
            other => panic!("expected file_edit to reject stale edit, got: {other:?}"),
        }

        // A rejected edit must leave the user's version untouched.
        assert_eq!(
            std::fs::read_to_string(file_path).expect("read final file"),
            USER_EDIT
        );
    }
}
