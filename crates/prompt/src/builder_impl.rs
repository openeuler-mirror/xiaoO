use agent_contracts::{PromptBuildInput, PromptBuilder, ToolSpecView};
use agent_llm::ChatMessageExt;
use agent_types::context::prompt::{PromptBuildError, PromptBuildResult};
use agent_types::{ChatMessage, ContentBlock, LlmRequest, ResponseFormat, Tool, ToolChoice};
use async_trait::async_trait;

use crate::compose::compose_system_text;
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
        let system_text = compose_system_text(&input.system_prompt, &context, &input.visible_tools);

        if system_text.trim().is_empty() {
            return Err(PromptBuildError::BuildFailed {
                message: "missing required context: system_prompt".to_string(),
            });
        }

        let mut messages = Vec::with_capacity(input.messages.len() + 1);
        messages.push(ChatMessage::system(system_text.clone()));
        messages.extend(input.messages);

        let request = LlmRequest {
            messages,
            tools: project_tools(&input.visible_tools),
            tool_choice: decision.tool_choice,
            max_tokens: Some(input.budget.reserved_for_output),
            temperature: None,
            response_format: decision.response_format,
        };

        Ok(PromptBuildResult {
            estimated_input_tokens: estimate_request_size(&request),
            request,
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
    visible_tools
        .iter()
        .map(|tool| Tool {
            name: tool.name().0.clone(),
            description: tool.description().to_string(),
            parameters: tool.input_schema().schema.clone(),
        })
        .collect()
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
mod tests {
    use super::*;

    use agent_types::common::ids::{ToolId, ToolName};
    use agent_types::context::features::FeatureFlags;
    use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
    use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
    use agent_types::TokenBudgetConfig;

    struct TestToolSpec {
        id: ToolId,
        name: ToolName,
        description: String,
        input_schema: InputSchemaRef,
    }

    impl ToolSpecView for TestToolSpec {
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
            static DEFAULT: OutputContract = OutputContract {
                description: String::new(),
            };
            &DEFAULT
        }
        fn effect_profile(&self) -> &EffectProfile {
            static DEFAULT: EffectProfile = EffectProfile {
                reads_filesystem: false,
                writes_filesystem: false,
                network_access: false,
                side_effects: false,
            };
            &DEFAULT
        }
    }

    #[test]
    #[ignore]
    fn build_projects_tools_into_llm_request() {
        let builder = PromptBuilderImpl::new();
        let input = PromptBuildInput {
            system_prompt: "You are a coding agent.".to_string(),
            messages: vec![ChatMessage::user("hello")],
            visible_tools: vec![std::sync::Arc::new(TestToolSpec {
                id: ToolId("search".to_string()),
                name: ToolName("search".to_string()),
                description: "search docs".to_string(),
                input_schema: InputSchemaRef {
                    schema: serde_json::json!({
                        "type": "object",
                        "properties": {"query": {"type": "string"}}
                    }),
                },
            }) as std::sync::Arc<dyn ToolSpecView>],
            skill_summaries: vec![SkillSummary {
                skill_id: "skill".to_string(),
                description: "do thing".to_string(),
            }],
            memory_snippets: vec![MemorySnippet {
                source: "memory".to_string(),
                content: "remember this".to_string(),
                relevance_score: 0.9,
            }],
            environment: EnvironmentInfo {
                model: "gpt-test".to_string(),
                cwd: "/tmp".to_string(),
                workspace_root: None,
                date: "2026-04-10".to_string(),
                agent_id: "main".to_string(),
            },
            feature_flags: FeatureFlags::default(),
            turn_count: 1,
            budget: TokenBudgetConfig {
                total_budget: 1024,
                reserved_for_output: 256,
                reserved_for_system: 128,
                hard_limit_ratio: 0.9,
            },
        };

        let result = futures::executor::block_on(builder.build(input)).unwrap();

        assert_eq!(result.request.tools.len(), 1);
        assert!(matches!(result.request.tool_choice, ToolChoice::Auto));
        assert!(matches!(
            result.request.response_format,
            ResponseFormat::Text
        ));
        assert_eq!(result.request.max_tokens, Some(256));
        assert!(matches!(
            result.request.messages[0].role,
            agent_types::MessageRole::System
        ));
        assert!(result.estimated_input_tokens > 0);
    }

    #[test]
    #[ignore]
    fn build_fails_fast_when_budget_is_zero() {
        let builder = PromptBuilderImpl::new();
        let input = PromptBuildInput {
            system_prompt: "You are a coding agent.".to_string(),
            messages: vec![ChatMessage::user("hello")],
            visible_tools: Vec::new(),
            skill_summaries: Vec::new(),
            memory_snippets: Vec::new(),
            environment: EnvironmentInfo {
                model: "gpt-test".to_string(),
                cwd: String::new(),
                workspace_root: None,
                date: "2026-04-10".to_string(),
                agent_id: "main".to_string(),
            },
            feature_flags: FeatureFlags::default(),
            turn_count: 1,
            budget: TokenBudgetConfig {
                total_budget: 0,
                reserved_for_output: 0,
                reserved_for_system: 0,
                hard_limit_ratio: 1.0,
            },
        };

        let err = match futures::executor::block_on(builder.build(input)) {
            Ok(_) => panic!("expected prompt build to fail when budget is zero"),
            Err(err) => err,
        };

        assert!(matches!(err, PromptBuildError::BuildFailed { .. }));
    }
}
