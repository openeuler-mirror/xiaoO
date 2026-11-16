use std::sync::Arc;

use crate::tool::ToolSpecView;
use agent_types::context::prompt::{EnvironmentInfo, MemorySnippet, SkillSummary};
use agent_types::{ChatMessage, FeatureFlags, TokenBudgetConfig};

pub struct PromptBuildInput {
    pub system_prompt: String,
    pub messages: Vec<ChatMessage>,
    pub visible_tools: Vec<Arc<dyn ToolSpecView>>,
    /// skill 清单快照（registry 共享形态，`Arc` 克隆零深拷贝）
    pub skill_summaries: Arc<[SkillSummary]>,
    pub memory_snippets: Vec<MemorySnippet>,
    pub environment: EnvironmentInfo,
    pub feature_flags: FeatureFlags,
    pub turn_count: u32,
    pub budget: TokenBudgetConfig,
}
