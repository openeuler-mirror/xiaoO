use agent_types::AssistantMessage;

pub trait AssistantMessageExt {
    fn has_tool_calls(&self) -> bool;
    fn is_text_only(&self) -> bool;
}

impl AssistantMessageExt for AssistantMessage {
    fn has_tool_calls(&self) -> bool {
        !self.tool_calls.is_empty()
    }

    fn is_text_only(&self) -> bool {
        self.tool_calls.is_empty() && self.text.is_some()
    }
}
