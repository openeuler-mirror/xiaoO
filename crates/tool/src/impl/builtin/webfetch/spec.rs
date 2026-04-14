use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use super::constants::{default_timeout_ms, max_timeout_ms};

#[derive(Debug, Clone)]
pub struct WebFetchToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl WebFetchToolSpec {
    pub fn new() -> Self {
        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from. Must start with http:// or https://"
                },
                "format": {
                    "type": "string",
                    "enum": ["text", "markdown", "html"],
                    "description": "The format to return the content in. Use 'text' for plain text, 'markdown' for Markdown-formatted content, or 'html' for raw HTML"
                },
                "timeout": {
                    "type": "number",
                    "description": format!(
                        "Optional timeout in milliseconds. Defaults to {}ms and may not exceed {}ms",
                        default_timeout_ms(),
                        max_timeout_ms()
                    )
                }
            },
            "required": ["url", "format"]
        });

        Self {
            id: ToolId("builtin_webfetch".to_string()),
            name: ToolName("webfetch".to_string()),
            description: "Fetches content from a URL and returns it as text, markdown, or raw HTML. Supports http and https URLs. Response size is limited to 5MB.".to_string(),
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Fetched web content including the content string, URL, content-type header, and the format used".to_string(),
            },
            effect_profile: EffectProfile {
                reads_filesystem: false,
                writes_filesystem: false,
                network_access: true,
                side_effects: false,
            },
        }
    }
}

impl Default for WebFetchToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for WebFetchToolSpec {
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
