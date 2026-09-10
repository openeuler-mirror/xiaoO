use super::*;
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
use agent_types::events::LoopEndSummary;
use agent_types::tool::execution_types::{ToolExecutionError, ToolExecutorOutput};
use agent_types::tool::spec_types::{EffectProfile, InputSchemaRef, OutputContract};
use agent_types::tool::FinalToolCall;
use agent_types::{
    AssistantMessage, LlmError, LlmRequest, LlmResponse, StopReason, StreamChunk, ToolUseBlock,
    Usage,
};
use async_trait::async_trait;
use llm_client::LlmProviderWrapper;
use tool::EmptyToolRegistry;

use crate::runtime_support::{EmptySkillRegistry, NoopRuntimeView};

include!("basics_test.rs");
include!("runtime_test.rs");
include!("tools_test.rs");
include!("streaming_test.rs");
include!("truncation_test.rs");
