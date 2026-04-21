use std::path::Path;

use agent_types::lsp::{
    LspCallHierarchyItem, LspDiagnostic, LspError, LspIncomingCall, LspLocation, LspOutgoingCall,
    LspSymbol,
};
use async_trait::async_trait;

/// Abstraction over a running LSP service.
///
/// Implemented by the concrete `lsp::LspService` struct.  Consumers (tools,
/// daemon runtime) depend only on this trait so the lsp crate is not a
/// hard compile-time dependency where it isn't needed.
#[async_trait]
pub trait LspProvider: Send + Sync {
    /// Diagnostics (errors / warnings) for `file`.
    async fn diagnostics(&self, file: &Path) -> Result<Vec<LspDiagnostic>, LspError>;

    /// Hover info (type, docs) at a 1-based position.
    async fn hover(&self, file: &Path, line: u32, col: u32) -> Result<Option<String>, LspError>;

    /// Go-to-definition: source locations of the symbol at `(line, col)`.
    async fn definition(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError>;

    /// All references of the symbol at `(line, col)`.
    async fn references(
        &self,
        file: &Path,
        line: u32,
        col: u32,
        include_declaration: bool,
    ) -> Result<Vec<LspLocation>, LspError>;

    /// Document symbols (`query = None`) or workspace symbol search (`query = Some(q)`).
    async fn symbols(
        &self,
        file: &Path,
        query: Option<&str>,
    ) -> Result<Vec<LspSymbol>, LspError>;

    /// Go-to-implementation: source locations of the concrete implementation(s).
    async fn implementation(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspLocation>, LspError>;

    /// Call hierarchy item at the given position (prerequisite for incoming/outgoing calls).
    async fn prepare_call_hierarchy(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspCallHierarchyItem>, LspError>;

    /// All callers of the function at the given position.
    async fn incoming_calls(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspIncomingCall>, LspError>;

    /// All functions called by the function at the given position.
    async fn outgoing_calls(
        &self,
        file: &Path,
        line: u32,
        col: u32,
    ) -> Result<Vec<LspOutgoingCall>, LspError>;

    /// Open `file` in the LSP server without waiting for any result.
    ///
    /// This sends `textDocument/didOpen` (and starts the server if needed) so
    /// that subsequent hover / definition / diagnostics calls return faster.
    /// Errors are silently ignored — callers must not depend on this succeeding.
    async fn touch_file(&self, file: &Path);
}
