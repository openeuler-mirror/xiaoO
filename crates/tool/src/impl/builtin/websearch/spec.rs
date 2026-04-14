use agent_contracts::tool::ToolSpecView;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};

use super::constants::{DEFAULT_NUM_RESULTS, MAX_NUM_RESULTS};

#[derive(Debug, Clone)]
pub struct WebSearchToolSpec {
    id: ToolId,
    name: ToolName,
    description: String,
    input_schema: InputSchemaRef,
    output_contract: OutputContract,
    effect_profile: EffectProfile,
}

impl WebSearchToolSpec {
    pub fn new() -> Self {
        let current_year = 2026; // updated at build time; LLM description references current year
        let description = format!(
            "Searches the web using Exa and returns relevant results with full page content. \
            Useful for finding up-to-date information, recent events (up to {}), documentation, \
            news, and any topic that may not be covered in the model's training data.",
            current_year
        );

        let schema = serde_json::json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The web search query string."
                },
                "num_results": {
                    "type": "number",
                    "description": format!(
                        "Number of search results to return. Defaults to {}. Maximum is {}.",
                        DEFAULT_NUM_RESULTS, MAX_NUM_RESULTS
                    )
                },
                "livecrawl": {
                    "type": "string",
                    "enum": ["fallback", "preferred"],
                    "description": "Live crawl mode. 'fallback': use live crawling as backup if cached content is unavailable. 'preferred': prioritize live crawling for fresher content. Defaults to 'fallback'."
                },
                "search_type": {
                    "type": "string",
                    "enum": ["auto", "fast", "deep"],
                    "description": "Search type. 'auto': balanced search (default). 'fast': optimized for quick results. 'deep': comprehensive search for harder queries."
                },
                "context_max_characters": {
                    "type": "number",
                    "description": "Maximum number of characters for the context string returned per result, optimized for LLM consumption. Defaults to 10000."
                }
            },
            "required": ["query"]
        });

        Self {
            id: ToolId("builtin_web_search".to_string()),
            name: ToolName("web_search".to_string()),
            description,
            input_schema: InputSchemaRef { schema },
            output_contract: OutputContract {
                description: "Search results as a text string, along with the original query."
                    .to_string(),
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

impl Default for WebSearchToolSpec {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolSpecView for WebSearchToolSpec {
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
