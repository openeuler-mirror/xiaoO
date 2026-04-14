use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use agent_contracts::runtime::runtime_view::RuntimeView;
use agent_contracts::tool::{ToolExecutor, ToolSpecView};
use agent_types::tool::call_types::FinalToolCall;
use agent_types::tool::execution_types::{RawToolOutcome, ToolExecutionError};

use super::constants::{
    BASE_URL, DEFAULT_NUM_RESULTS, DEFAULT_TIMEOUT_MS, MCP_TOOL_NAME, SEARCH_ENDPOINT,
};
use super::input::{LivecrawlMode, SearchType, WebSearchInput};
use super::output::WebSearchOutput;
use super::spec::WebSearchToolSpec;
use super::validation;

// JSON-RPC request types
#[derive(Debug, Serialize)]
struct McpSearchArguments {
    query: String,
    #[serde(rename = "type")]
    search_type: String,
    #[serde(rename = "numResults")]
    num_results: u32,
    livecrawl: String,
    #[serde(
        rename = "contextMaxCharacters",
        skip_serializing_if = "Option::is_none"
    )]
    context_max_characters: Option<u32>,
}

#[derive(Debug, Serialize)]
struct McpSearchParams {
    name: String,
    arguments: McpSearchArguments,
}

#[derive(Debug, Serialize)]
struct McpSearchRequest {
    jsonrpc: String,
    id: u32,
    method: String,
    params: McpSearchParams,
}

// JSON-RPC response types
#[derive(Debug, Deserialize)]
struct McpContentItem {
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

#[derive(Debug, Deserialize)]
struct McpResult {
    content: Vec<McpContentItem>,
}

#[derive(Debug, Deserialize)]
struct McpSearchResponse {
    result: Option<McpResult>,
}

pub struct WebSearchExecutor {
    spec: Arc<WebSearchToolSpec>,
}

impl WebSearchExecutor {
    pub fn new(spec: Arc<WebSearchToolSpec>) -> Self {
        Self { spec }
    }

    async fn search(input: &WebSearchInput) -> Result<WebSearchOutput, String> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        let search_type = input
            .search_type
            .as_ref()
            .map(|t| t.to_string())
            .unwrap_or_else(|| SearchType::default().to_string());

        let livecrawl = input
            .livecrawl
            .as_ref()
            .map(|l| l.to_string())
            .unwrap_or_else(|| LivecrawlMode::default().to_string());

        let request_body = McpSearchRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "tools/call".to_string(),
            params: McpSearchParams {
                name: MCP_TOOL_NAME.to_string(),
                arguments: McpSearchArguments {
                    query: input.query.clone(),
                    search_type,
                    num_results: input.num_results.unwrap_or(DEFAULT_NUM_RESULTS),
                    livecrawl,
                    context_max_characters: input.context_max_characters,
                },
            },
        };

        let url = format!("{}{}", BASE_URL, SEARCH_ENDPOINT);

        let response = client
            .post(&url)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .json(&request_body)
            .send()
            .await
            .map_err(|e| format!("Search request failed: {}", e))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "(failed to read error body)".to_string());
            return Err(format!("Search error ({}): {}", status, error_text));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| format!("Failed to read response body: {}", e))?;

        // Parse SSE response: each event line starts with "data: "
        for line in response_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(parsed) = serde_json::from_str::<McpSearchResponse>(data) {
                    if let Some(result) = parsed.result {
                        if let Some(item) = result
                            .content
                            .into_iter()
                            .find(|c| c.content_type == "text")
                        {
                            return Ok(WebSearchOutput {
                                result: item.text,
                                query: input.query.clone(),
                            });
                        }
                    }
                }
            }
        }

        // Also try parsing as plain JSON (non-SSE response)
        if let Ok(parsed) = serde_json::from_str::<McpSearchResponse>(&response_text) {
            if let Some(result) = parsed.result {
                if let Some(item) = result
                    .content
                    .into_iter()
                    .find(|c| c.content_type == "text")
                {
                    return Ok(WebSearchOutput {
                        result: item.text,
                        query: input.query.clone(),
                    });
                }
            }
        }

        Ok(WebSearchOutput {
            result: "No search results found. Please try a different query.".to_string(),
            query: input.query.clone(),
        })
    }
}

impl Default for WebSearchExecutor {
    fn default() -> Self {
        Self::new(Arc::new(WebSearchToolSpec::new()))
    }
}

#[async_trait]
impl ToolExecutor for WebSearchExecutor {
    fn spec(&self) -> &dyn ToolSpecView {
        self.spec.as_ref()
    }

    async fn invoke(
        &self,
        call: &FinalToolCall,
        _runtime: &dyn RuntimeView,
    ) -> Result<RawToolOutcome, ToolExecutionError> {
        let input: WebSearchInput = serde_json::from_value(call.input.clone()).map_err(|e| {
            ToolExecutionError::ExecutionFailed {
                message: format!("Failed to parse input: {}", e),
            }
        })?;

        let validation_result = validation::validate_input(&input);
        if !validation_result.result {
            let error_message = validation_result
                .message
                .unwrap_or_else(|| "Validation failed".to_string());
            let error_code = validation_result.error_code.unwrap_or(0);
            return Ok(RawToolOutcome::Error {
                message: format!("[error_code={}] {}", error_code, error_message),
            });
        }

        let output = Self::search(&input)
            .await
            .map_err(|message| ToolExecutionError::ExecutionFailed { message })?;

        let serialized =
            serde_json::to_string(&output).map_err(|e| ToolExecutionError::ExecutionFailed {
                message: format!("Failed to serialize output: {}", e),
            })?;

        Ok(RawToolOutcome::Success { output: serialized })
    }
}
