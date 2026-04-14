use agent_contracts::tool::{ToolCall, ToolCallBuilder, ToolFilter, ToolCallFactory};
use agent_types::common::BuildError;
use agent_types::tool::RawToolCall;

use super::ToolCallBuilderImpl;

#[derive(Debug, Default, Clone, Copy)]
pub struct ToolCallFactoryImpl;

impl ToolCallFactoryImpl {
    pub fn new() -> Self {
        Self
    }
}

impl ToolCallFactory for ToolCallFactoryImpl {
    fn create_tool_call(
        &self,
        raw: RawToolCall,
        filter: Box<dyn ToolFilter>,
    ) -> Result<Box<dyn ToolCall>, BuildError> {
        ToolCallBuilderImpl::new()
            .with_raw_llm_tool_call(raw)
            .with_tool_filter(filter)
            .build()
    }
}
