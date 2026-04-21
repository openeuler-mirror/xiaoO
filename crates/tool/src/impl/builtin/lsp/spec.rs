use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

#[derive(Debug, Clone)]
pub struct LspToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl LspToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["diagnostics", "hover", "definition", "references", "symbols", "implementation", "prepare_call_hierarchy", "incoming_calls", "outgoing_calls"],
                    "description": "LSP action:\n- diagnostics: compiler errors/warnings for a file\n- hover: type info and docs at a position\n- definition: where a symbol is defined\n- references: all usages of a symbol\n- symbols: list symbols in a file, or search workspace symbols by query\n- implementation: concrete implementations of an interface or abstract method\n- prepare_call_hierarchy: call hierarchy item at position (name, kind, location)\n- incoming_calls: all callers of the function at position\n- outgoing_calls: all functions called by the function at position"
                },
                "file_path": {
                    "type": "string",
                    "description": "Absolute or workspace-relative path to the source file. Required for all actions."
                },
                "line": {
                    "type": "integer",
                    "description": "1-based line number. Required for: hover, definition, references."
                },
                "column": {
                    "type": "integer",
                    "description": "1-based column number. Required for: hover, definition, references."
                },
                "query": {
                    "type": "string",
                    "description": "Symbol name filter for 'symbols' action. Triggers workspace-wide search. Omit to list all symbols in file_path."
                },
                "include_declaration": {
                    "type": "boolean",
                    "description": "For 'references': include the declaration site in results. Default: true."
                }
            },
            "required": ["action", "file_path"]
        });

        Self {
            id: ToolId("builtin_lsp".to_string()),
            name: ToolName("lsp".to_string()),
            description: "Query language server (LSP) for code diagnostics, symbol info, navigation, and call hierarchy.".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "LSP query result".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: true,
                writes_filesystem: false,
                network_access: false,
                side_effects: false,
            },
        }
    }
}

impl Default for LspToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for LspToolSpec {
    fn id(&self) -> &ToolId {
        &self.id
    }

    fn name(&self) -> &ToolName {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> &InputSchemaRef {
        &self.input_schema
    }

    fn output_contract(&self) -> &OutputContract {
        &self.output_contract
    }

    fn effect_profile(&self) -> &EffectProfile {
        &self.effect_profile
    }
}
