use crate::backends::local::backend::LocalOperationBackend;
use agent_contracts::backend::OperationBackend;
use std::sync::Arc;

/// Build a minimal local backend for the LSP subsystem.
///
/// Uses the process home directory as workspace root. Suitable for
/// daemon-level singletons that need file / exec operations independently
/// of any per-session backend.
pub fn local_lsp_backend() -> Arc<dyn OperationBackend> {
    Arc::new(LocalOperationBackend::new_with_home())
}
