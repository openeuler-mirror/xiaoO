use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Source code position (1-based, as exposed to tools and callers).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct LspPosition {
    pub line: u32,
    pub col: u32,
}

/// A resolved source location returned by definition / references queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspLocation {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// Diagnostic severity, mirroring the LSP protocol values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

impl Severity {
    pub fn from_lsp_code(code: u32) -> Self {
        match code {
            1 => Severity::Error,
            2 => Severity::Warning,
            3 => Severity::Information,
            _ => Severity::Hint,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Information => "information",
            Severity::Hint => "hint",
        }
    }
}

/// A compiler / linter diagnostic emitted by a language server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspDiagnostic {
    pub severity: String,
    pub line: u32,
    pub col: u32,
    pub message: String,
    pub source: Option<String>,
    pub code: Option<String>,
}

/// A symbol (function, struct, variable, …) returned by document / workspace symbol queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspSymbol {
    pub name: String,
    pub kind: String,
    pub location: LspLocation,
    pub container: Option<String>,
}

/// A call hierarchy item — represents a function / method at a specific location.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspCallHierarchyItem {
    pub name: String,
    pub kind: String,
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// One caller of a queried function, with the exact call sites in the caller's file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspIncomingCall {
    pub caller: LspCallHierarchyItem,
    pub call_sites: Vec<LspLocation>,
}

/// One function called by the queried function, with the call sites in the current file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LspOutgoingCall {
    pub callee: LspCallHierarchyItem,
    pub call_sites: Vec<LspLocation>,
}

/// Errors that can occur when interacting with a language server.
#[derive(Debug, Error)]
pub enum LspError {
    #[error("no language server found for file: {0}")]
    NoServerForFile(String),

    #[error("server failed to start: {0}")]
    StartupFailed(String),

    #[error("server not running")]
    NotRunning,

    #[error("server permanently failed: {0}")]
    PermanentlyFailed(String),

    #[error("request timed out")]
    Timeout,

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("rpc error {code}: {message}")]
    Rpc { code: i64, message: String },
}
