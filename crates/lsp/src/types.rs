// Public types live in agent-types::lsp; re-export them for convenience.
pub use agent_types::lsp::{LspDiagnostic, LspLocation, LspPosition, LspSymbol, Severity};

// ── Internal LSP protocol helpers (not part of the public API) ───────────────

/// LSP 0-based position used only in JSON-RPC messages.
#[derive(Debug, Clone)]
pub(crate) struct LspPos {
    pub line: u32,
    pub character: u32,
}

impl LspPos {
    /// Convert from 1-based tool position to 0-based LSP position.
    pub fn from_1based(line: u32, col: u32) -> Self {
        Self {
            line: line.saturating_sub(1),
            character: col.saturating_sub(1),
        }
    }
}

pub(crate) fn symbol_kind_name(kind: u32) -> &'static str {
    match kind {
        1 => "file",
        2 => "module",
        3 => "namespace",
        4 => "package",
        5 => "class",
        6 => "method",
        7 => "property",
        8 => "field",
        9 => "constructor",
        10 => "enum",
        11 => "interface",
        12 => "function",
        13 => "variable",
        14 => "constant",
        15 => "string",
        16 => "number",
        17 => "boolean",
        18 => "array",
        19 => "object",
        20 => "key",
        21 => "null",
        22 => "enum_member",
        23 => "struct",
        24 => "event",
        25 => "operator",
        26 => "type_parameter",
        _ => "unknown",
    }
}
