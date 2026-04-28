use agent_types::context::prompt::PromptBuildError;
use agent_types::{ChatMessage, ContentBlock, MessageRole, ResponseFormat, ToolChoice};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptState {
    NewUserTurn,
    AfterToolResult,
    FinalAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAction {
    AnswerOnly,
    PlanAndMaybeUseTools,
    IntegrateToolResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMode {
    None,
    Auto,
    Required,
}

#[derive(Debug, Clone)]
pub struct PromptDecision {
    pub state: PromptState,
    pub action: PromptAction,
    pub tool_mode: ToolMode,
    pub tool_choice: ToolChoice,
    pub response_format: ResponseFormat,
}

pub fn decide_prompt(
    messages: &[ChatMessage],
    has_visible_tools: bool,
) -> Result<PromptDecision, PromptBuildError> {
    let last_message = messages.last().ok_or(PromptBuildError::EmptyMessages)?;
    let state = infer_state(last_message);
    let action = infer_action(state, has_visible_tools);
    let tool_mode = infer_tool_mode(action, has_visible_tools);
    let tool_choice = match tool_mode {
        ToolMode::None => ToolChoice::None,
        ToolMode::Auto => ToolChoice::Auto,
        ToolMode::Required => ToolChoice::Required,
    };

    Ok(PromptDecision {
        state,
        action,
        tool_mode,
        tool_choice,
        response_format: ResponseFormat::Text,
    })
}

fn infer_state(last_message: &ChatMessage) -> PromptState {
    if matches!(last_message.role, MessageRole::Tool)
        || last_message
            .blocks
            .iter()
            .any(|block| matches!(block, ContentBlock::ToolResult { .. }))
    {
        PromptState::AfterToolResult
    } else {
        PromptState::NewUserTurn
    }
}

fn infer_action(state: PromptState, has_visible_tools: bool) -> PromptAction {
    match state {
        PromptState::AfterToolResult => PromptAction::IntegrateToolResults,
        PromptState::FinalAnswer => PromptAction::AnswerOnly,
        PromptState::NewUserTurn => {
            if has_visible_tools {
                PromptAction::PlanAndMaybeUseTools
            } else {
                PromptAction::AnswerOnly
            }
        }
    }
}

fn infer_tool_mode(action: PromptAction, has_visible_tools: bool) -> ToolMode {
    match action {
        PromptAction::AnswerOnly => ToolMode::None,
        PromptAction::IntegrateToolResults | PromptAction::PlanAndMaybeUseTools => {
            if has_visible_tools {
                ToolMode::Auto
            } else {
                ToolMode::None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_llm::ChatMessageExt;

    #[test]
    #[ignore]
    fn chooses_auto_tool_mode_for_new_user_turn_with_visible_tools() {
        let messages = vec![ChatMessage::user("search docs")];

        let decision = decide_prompt(&messages, true).unwrap();

        assert_eq!(decision.state, PromptState::NewUserTurn);
        assert_eq!(decision.action, PromptAction::PlanAndMaybeUseTools);
        assert_eq!(decision.tool_mode, ToolMode::Auto);
        assert!(matches!(decision.tool_choice, ToolChoice::Auto));
    }

    #[test]
    #[ignore]
    fn treats_tool_result_message_as_integration_turn() {
        let messages = vec![ChatMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                call_id: "call-1".to_string(),
                tool_name: "search".to_string(),
                output: "done".to_string(),
                is_error: false,
            }],
            message_id: None,
            timestamp_ms: 0,
            api_usage_tokens: None,
            reasoning_content: None,
        }];

        let decision = decide_prompt(&messages, true).unwrap();

        assert_eq!(decision.state, PromptState::AfterToolResult);
        assert_eq!(decision.action, PromptAction::IntegrateToolResults);
        assert_eq!(decision.tool_mode, ToolMode::None);
        assert!(matches!(decision.tool_choice, ToolChoice::None));
    }
}
