use crate::tool::call::ToolCall;
use crate::tool::registry::ToolFilter;
use agent_types::common::BuildError;
use agent_types::tool::RawToolCall;

pub trait ToolCallFactory: Send + Sync {
    fn create_tool_call(
        &self,
        raw: RawToolCall,
        filter: Box<dyn ToolFilter>,
    ) -> Result<Box<dyn ToolCall>, BuildError>;
}
