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
#[path = "../../../tests/unit/prompt/decision_test.rs"]
mod tests;
