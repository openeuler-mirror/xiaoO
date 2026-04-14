use std::sync::Arc;

use agent_contracts::context::budget::TokenBudgetPolicy;
use agent_contracts::{CompressionPipeline, PromptBuilder, SkillRegistry, ToolRegistry};
use agent_types::context::{FeatureFlags, TokenBudgetConfig};
use llm_client::LlmProviderWrapper;

pub struct RuntimeSnapshot {
    pub llm_provider: Arc<LlmProviderWrapper>,
    pub tool_registry: Arc<dyn ToolRegistry>,
    pub skill_registry: Arc<dyn SkillRegistry>,
    pub prompt_builder: Arc<dyn PromptBuilder>,
    pub system_prompt: Arc<str>,
    pub feature_flags: FeatureFlags,

    pub compression_pipeline: Arc<dyn CompressionPipeline>,
    pub max_turns: u32,
    pub token_budget_config: TokenBudgetConfig,
    pub token_budget_policy: Arc<dyn TokenBudgetPolicy>,
}
