use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use agent_contracts::lsp::LspProvider;
use agent_types::lsp::LspDiagnostic;
use serde::{Deserialize, Serialize};

/// Compact diagnostics snapshot embedded in file-tool output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnosticsInfo {
    pub has_errors: bool,
    pub count: usize,
    pub items: Vec<LspDiagnostic>,
}

/// Spawn a background task that calls `lsp.touch_file(file)`.
///
/// Returns immediately — the caller is never blocked. Mirrors opencode's
/// `LSP.touchFile(filepath, false)` in `read.ts`: send `textDocument/didOpen`
/// so the language server starts indexing the file ahead of any future request.
pub fn spawn_touch_file(lsp: &Arc<dyn LspProvider>, file: &Path) {
    let lsp = Arc::clone(lsp);
    let path = PathBuf::from(file);
    tokio::spawn(async move {
        lsp.touch_file(&path).await;
    });
}

/// Call `lsp.diagnostics(file)` with a wall-clock timeout.
/// Returns `None` if LSP is not available, the file type is not supported,
/// or the call times out.
pub async fn fetch_diagnostics(
    lsp: &Arc<dyn LspProvider>,
    file: &Path,
    timeout_secs: u64,
) -> Option<LspDiagnosticsInfo> {
    let result = tokio::time::timeout(
        Duration::from_secs(timeout_secs),
        lsp.diagnostics(file),
    )
    .await;

    match result {
        Ok(Ok(items)) => {
            let has_errors = items.iter().any(|d| d.severity == "error");
            let count = items.len();
            Some(LspDiagnosticsInfo { has_errors, count, items })
        }
        Ok(Err(_)) | Err(_) => None,
    }
}
