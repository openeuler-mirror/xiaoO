use agent_contracts::{PromptBuildInput, PromptBuilder, ToolSpecView};
use agent_llm::ChatMessageExt;
use agent_types::context::prompt::{PromptBuildError, PromptBuildResult};
use agent_types::{
    ChatMessage, ContentBlock, LlmRequest, MessageRole, ReasoningEffort, ResponseFormat, Tool,
    ToolChoice,
};
use async_trait::async_trait;

use crate::compose::compose_system_sections;
use crate::context::collect_prompt_context;
use crate::decision::decide_prompt;

#[derive(Debug, Default, Clone, Copy)]
pub struct PromptBuilderImpl;

impl PromptBuilderImpl {
    pub fn new() -> Self {
        Self
    }

    fn build_inner(
        &self,
        mut input: PromptBuildInput,
    ) -> Result<PromptBuildResult, PromptBuildError> {
        validate_input(&input)?;

        if !input.feature_flags.skill_matching {
            input.skill_summaries.clear();
        }

        let decision = decide_prompt(&input.messages, !input.visible_tools.is_empty())?;
        let context = collect_prompt_context(&input);
        // Compose the system prompt: cache-stable parts only (base, workspace,
        // skills, repo map, environment), un-joined. They feed both the joined
        // system message and the `system_parts` array exposed to the
        // `*.Chat.system.transform` hooker. Per-turn-volatile context is NOT
        // part of the system message — see the reminder injection below.
        let system_parts = compose_system_sections(&input.system_prompt, &context);
        let system_text = system_parts.join("\n\n");

        if system_text.trim().is_empty() {
            return Err(PromptBuildError::BuildFailed {
                message: "missing required context: system_prompt".to_string(),
            });
        }

        let mut messages = Vec::with_capacity(input.messages.len() + 2);
        messages.push(ChatMessage::system(system_text));

        messages.extend(
            input
                .messages
                .iter()
                .filter(|m| m.role != MessageRole::System)
                .cloned(),
        );

        // Per-turn-volatile context (horizon, plan, memory) rides in an
        // ephemeral `<system-reminder>` user message appended at the very END
        // of the request, never persisted into session history. Rationale:
        // the request token stream is [system, history..., reminder]; putting
        // per-turn churn in the system tail would invalidate provider prefix
        // caches for the whole history every turn, while a tail message keeps
        // everything before it byte-identical. A dedicated user message (not
        // a text block on the last history message) is required because the
        // OpenAI-family wire conversion collapses a Tool message to a single
        // `content` field — an extra text block there would clobber the tool
        // result.
        if let Some(reminder) =
            crate::compose::compose_turn_context_reminder(&context).filter(|r| !r.trim().is_empty())
        {
            messages.push(ChatMessage::user(reminder));
        }

        let request = LlmRequest {
            messages,
            tools: project_tools(&input.visible_tools),
            tool_choice: decision.tool_choice,
            max_tokens: Some(input.budget.reserved_for_output),
            temperature: None,
            response_format: decision.response_format,
            reasoning_effort: ReasoningEffort::Off,
        };

        Ok(PromptBuildResult {
            estimated_input_tokens: estimate_request_size(&request),
            request,
            system_parts,
        })
    }
}

#[async_trait]
impl PromptBuilder for PromptBuilderImpl {
    async fn build(&self, input: PromptBuildInput) -> Result<PromptBuildResult, PromptBuildError> {
        self.build_inner(input)
    }
}

fn validate_input(input: &PromptBuildInput) -> Result<(), PromptBuildError> {
    if input.system_prompt.trim().is_empty() {
        return Err(PromptBuildError::BuildFailed {
            message: "missing required context: system_prompt".to_string(),
        });
    }

    if input.messages.is_empty() {
        return Err(PromptBuildError::EmptyMessages);
    }

    if input.budget.total_budget == 0 {
        return Err(PromptBuildError::BuildFailed {
            message: "prompt budget exhausted".to_string(),
        });
    }

    if !input.feature_flags.tool_execution && !input.visible_tools.is_empty() {
        return Err(PromptBuildError::BuildFailed {
            message: "invalid prompt state: visible tools provided while tool execution feature is disabled"
                .to_string(),
        });
    }

    Ok(())
}

fn project_tools(visible_tools: &[std::sync::Arc<dyn ToolSpecView>]) -> Vec<Tool> {
    let mut tools: Vec<Tool> = visible_tools
        .iter()
        .map(|tool| Tool {
            name: tool.name().0.clone(),
            description: tool.description().to_string(),
            parameters: tool.input_schema().schema.clone(),
        })
        .collect();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

fn estimate_request_size(request: &LlmRequest) -> usize {
    let message_size = request
        .messages
        .iter()
        .map(|message| {
            message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.len(),
                    ContentBlock::ToolUse {
                        call_id,
                        tool_name,
                        input,
                    } => {
                        call_id.len()
                            + tool_name.len()
                            + serde_json::to_string(input).unwrap_or_default().len()
                    }
                    ContentBlock::ToolResult {
                        call_id,
                        tool_name,
                        output,
                        ..
                    } => call_id.len() + tool_name.len() + output.len(),
                    ContentBlock::Image { description }
                    | ContentBlock::Document { description } => description.len(),
                })
                .sum::<usize>()
        })
        .sum::<usize>();

    let tool_size = request
        .tools
        .iter()
        .map(|tool| {
            tool.name.len()
                + tool.description.len()
                + serde_json::to_string(&tool.parameters)
                    .unwrap_or_default()
                    .len()
        })
        .sum::<usize>();

    let response_format_size = match &request.response_format {
        ResponseFormat::Text | ResponseFormat::JsonObject => 0,
        ResponseFormat::JsonSchema { schema, .. } => {
            serde_json::to_string(schema).unwrap_or_default().len()
        }
    };

    let tool_choice_size = match &request.tool_choice {
        ToolChoice::Auto | ToolChoice::Required | ToolChoice::None => 0,
        ToolChoice::Specific(name) => name.len(),
    };

    message_size + tool_size + response_format_size + tool_choice_size
}

#[cfg(test)]
#[path = "../../../tests/unit/prompt/builder_impl_test.rs"]
mod tests;
