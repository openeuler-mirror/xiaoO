use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LspAction {
    Diagnostics,
    Hover,
    Definition,
    References,
    Symbols,
    Implementation,
    PrepareCallHierarchy,
    IncomingCalls,
    OutgoingCalls,
}

#[derive(Debug, Deserialize)]
pub struct LspInput {
    pub action: LspAction,
    pub file_path: String,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub query: Option<String>,
    #[serde(default = "default_true")]
    pub include_declaration: bool,
}

fn default_true() -> bool {
    true
}
