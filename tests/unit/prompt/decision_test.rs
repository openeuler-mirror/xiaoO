use super::*;
use agent_llm::ChatMessageExt;

#[test]
fn chooses_auto_tool_mode_for_new_user_turn_with_visible_tools() {
    let messages = vec![ChatMessage::user("search docs")];

    let decision = decide_prompt(&messages, true).unwrap();

    assert_eq!(decision.state, PromptState::NewUserTurn);
    assert_eq!(decision.action, PromptAction::PlanAndMaybeUseTools);
    assert_eq!(decision.tool_mode, ToolMode::Auto);
    assert!(matches!(decision.tool_choice, ToolChoice::Auto));
}

#[test]
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
        estimated_tokens: None,
    }];

    let decision = decide_prompt(&messages, true).unwrap();

    assert_eq!(decision.state, PromptState::AfterToolResult);
    assert_eq!(decision.action, PromptAction::IntegrateToolResults);
    assert_eq!(decision.tool_mode, ToolMode::Auto);
    assert!(matches!(decision.tool_choice, ToolChoice::Auto));
}
