use super::tool_exec::{
    char_boundary_before, filter_ask_user_question_output, is_parallel_safe, is_valid_tool_call,
    should_stop_after_tool_result, synthesize_missing_call_ids, truncate_tool_output,
    MAX_TOOL_OUTPUT_BYTES, MAX_TOOL_OUTPUT_LINES,
};
use super::*;
use crate::input::LoopStopRule;
use agent_types::tool::RawToolOutcome;
use std::sync::{Arc, Mutex as StdMutex};

use agent_contracts::context::budget::TokenBudgetPolicy;
use agent_contracts::tool::{ToolExecutor, ToolFilter, ToolRegistry, ToolSpecView};
use agent_contracts::{
    CompressionPipeline, LlmProvider, PromptBuilder, ProviderCapabilities, RuntimeView,
    SkillRegistry,
};
use agent_llm::LlmRequestExt;
use agent_types::common::ids::{ToolId, ToolName};
use agent_types::context::budget::BudgetError;
use agent_types::context::prompt::{PromptBuildError, PromptBuildResult};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use agent_types::events::{LoopEndSummary, ToolResultEvent};
use agent_types::tool::execution_types::{ToolExecutionError, ToolExecutorOutput};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use agent_types::tool::FinalToolCall;
use agent_types::{
    AssistantMessage, LlmError, LlmRequest, LlmResponse, StopReason, StreamChunk, ToolUseBlock,
    Usage,
};
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;
use tool::{tool_filter_from_specs, EmptyToolRegistry};

use crate::runtime_support::{EmptySkillRegistry, NoopRuntimeView};

include!("agent_loop/basics_test.rs");
include!("agent_loop/runtime_test.rs");
include!("agent_loop/tools_test.rs");
include!("agent_loop/streaming_test.rs");
include!("agent_loop/truncation_test.rs");
