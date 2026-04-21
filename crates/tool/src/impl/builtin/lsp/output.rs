use agent_types::lsp::{
    LspCallHierarchyItem, LspDiagnostic, LspIncomingCall, LspLocation, LspOutgoingCall, LspSymbol,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum LspOutput {
    Diagnostics {
        file: String,
        has_errors: bool,
        count: usize,
        items: Vec<LspDiagnostic>,
    },
    Hover {
        file: String,
        line: u32,
        column: u32,
        /// None means no information available at this position
        content: Option<String>,
    },
    Definition {
        locations: Vec<LspLocation>,
        count: usize,
    },
    References {
        locations: Vec<LspLocation>,
        count: usize,
    },
    Symbols {
        symbols: Vec<LspSymbol>,
        count: usize,
    },
    Implementation {
        locations: Vec<LspLocation>,
        count: usize,
    },
    PrepareCallHierarchy {
        items: Vec<LspCallHierarchyItem>,
        count: usize,
    },
    IncomingCalls {
        calls: Vec<LspIncomingCall>,
        count: usize,
    },
    OutgoingCalls {
        calls: Vec<LspOutgoingCall>,
        count: usize,
    },
}
