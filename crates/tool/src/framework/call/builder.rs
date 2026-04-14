use agent_contracts::tool::{ToolCall, ToolCallBuilder, ToolExecutor, ToolFilter, ToolSpecView};
use agent_types::common::BuildError;
use agent_types::tool::{FinalToolCall, RawToolCall};
use std::sync::Arc;

use super::ToolCallImpl;

pub struct ToolCallBuilderImpl {
    raw_tool_call: Option<RawToolCall>,
    tool_filter: Option<Box<dyn ToolFilter>>,
}

impl ToolCallBuilderImpl {
    pub fn new() -> Self {
        Self {
            raw_tool_call: None,
            tool_filter: None,
        }
    }

    fn resolve_visible_spec_by_name(
        tool_filter: &dyn ToolFilter,
        tool_name: &str,
    ) -> Option<Arc<dyn ToolSpecView>> {
        tool_filter.get_spec_for_name(tool_name)
    }

    fn resolve_visible_executor_by_name(
        tool_filter: &dyn ToolFilter,
        tool_name: &str,
    ) -> Option<Arc<dyn ToolExecutor>> {
        tool_filter.get_executor_for_name(tool_name)
    }
}

impl ToolCallBuilder for ToolCallBuilderImpl {
    fn with_raw_llm_tool_call(mut self, raw_tool_call: RawToolCall) -> Self {
        self.raw_tool_call = Some(raw_tool_call);
        self
    }

    fn with_tool_filter(mut self, tool_filter: Box<dyn ToolFilter>) -> Self {
        self.tool_filter = Some(tool_filter);
        self
    }

    fn build(self) -> Result<Box<dyn ToolCall>, BuildError> {
        let raw_tool_call = self
            .raw_tool_call
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "raw_tool_call".to_string(),
            })?;
        let tool_filter = self
            .tool_filter
            .ok_or_else(|| BuildError::MissingRequiredField {
                field: "tool_filter".to_string(),
            })?;

        let visible_spec =
            Self::resolve_visible_spec_by_name(tool_filter.as_ref(), &raw_tool_call.tool_name)
                .ok_or_else(|| BuildError::InvalidConfig {
                    message: format!(
                        "tool '{}' is not visible in the current ToolFilter",
                        raw_tool_call.tool_name
                    ),
                })?;

        let executor =
            Self::resolve_visible_executor_by_name(tool_filter.as_ref(), &raw_tool_call.tool_name)
                .ok_or_else(|| BuildError::InvalidConfig {
                    message: format!(
                        "tool '{}' is missing an executor in the current ToolFilter",
                        raw_tool_call.tool_name
                    ),
                })?;

        let _ = visible_spec.id();

        let final_call = FinalToolCall {
            call_id: raw_tool_call.call_id,
            tool_name: raw_tool_call.tool_name,
            input: raw_tool_call.input,
        };

        Ok(Box::new(ToolCallImpl::new(final_call, visible_spec, executor)) as Box<dyn ToolCall>)
    }
}
