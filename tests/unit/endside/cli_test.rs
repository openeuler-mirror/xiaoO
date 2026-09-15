use super::{resolve_effective_context_window, CliConfig};
use serde_json::Value;
use std::sync::Arc;
use xiaoo_api::llm::LlmProviderWrapper;
use xiaoo_shared::skills_support::SkillsConfig;
use xiaoo_shared::testing::stub_llm_provider;

fn test_config() -> CliConfig {
    CliConfig {
        skills_config: SkillsConfig::default(),
        kvcache_debug_enabled: false,
        provider: "openai".to_string(),
        model: "gpt-4.1".to_string(),
        api_key: None,
        api_key_env: None,
        api_base: None,
        trace: Value::Object(serde_json::Map::new()),
        system_prompt: "test".to_string(),
        max_turns: 1,
        enable_tools: false,
        visible_tools: None,
        reasoning_effort: Default::default(),
        kvcache_enabled: true,
        compact: crate::cli::config::CompactSection::default(),
        hooker: Default::default(),
        operation_backend: None,
        subagent: Default::default(),
        mcp_servers: Vec::new(),
        memory_automation: Default::default(),
    }
}

fn test_provider(max_context_window: usize) -> Arc<LlmProviderWrapper> {
    stub_llm_provider("dummy", max_context_window as u32)
}

#[tokio::test]
async fn cli_context_window_falls_back_to_provider_capability() {
    let mut config = test_config();
    config.provider = "unknown-provider".to_string();

    let resolved = resolve_effective_context_window(&config, &test_provider(12345)).await;
    assert_eq!(resolved, 12345);
}
