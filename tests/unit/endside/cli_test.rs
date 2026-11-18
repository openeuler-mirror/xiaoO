use super::{
    build_llm_provider, resolve_effective_context_window, validate_model_output_limit, CliConfig,
};
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
        request_headers: Default::default(),
        api_base: None,
        trace: Value::Object(serde_json::Map::new()),
        system_prompt: "test".to_string(),
        max_turns: 1,
        context_limit_tokens: None,
        max_output_tokens: None,
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

    let resolved = resolve_effective_context_window(&config, &test_provider(12345))
        .await
        .unwrap();
    assert_eq!(resolved, 12345);
}

#[tokio::test]
async fn cli_token_limits_validate_against_catalog() {
    let mut server = mockito::Server::new_async().await;
    let catalog = server
        .mock("GET", "/v1/models")
        .with_header("content-type", "application/json")
        .with_body(r#"{"data":[{"id":"token-budget-test","context_length":32768,"max_output_tokens":4096}]}"#)
        .expect_at_least(1)
        .create_async()
        .await;
    let mut config = test_config();
    config.model = "token-budget-test".into();
    config.api_base = Some(format!("{}/v1", server.url()));
    config.api_key = Some("loopback-key".into());

    let provider = build_llm_provider(&config, None).unwrap();
    assert_eq!(
        resolve_effective_context_window(&config, &provider)
            .await
            .unwrap(),
        32768
    );
    config.context_limit_tokens = Some(16384);
    assert_eq!(
        resolve_effective_context_window(&config, &provider)
            .await
            .unwrap(),
        16384
    );
    config.context_limit_tokens = Some(32769);
    assert!(resolve_effective_context_window(&config, &provider)
        .await
        .unwrap_err()
        .contains("32768"));
    config.max_output_tokens = Some(4097);
    assert!(validate_model_output_limit(&config)
        .await
        .unwrap_err()
        .contains("4096"));

    config.max_output_tokens = Some(512);
    validate_model_output_limit(&config).await.unwrap();
    catalog.assert_async().await;
}

#[test]
fn cli_rejects_output_limit_when_adapter_cannot_honor_it() {
    let mut config = test_config();
    config.provider = "ollama".into();
    config.max_output_tokens = Some(512);
    assert!(build_llm_provider(&config, None)
        .err()
        .unwrap()
        .to_string()
        .contains("unsupported"));

    config.provider = "anthropic".into();
    config.model = "claude-sonnet-4-20250514".into();
    config.api_key = Some("test-key".into());
    config.reasoning_effort = xiaoo_api::chat::ReasoningEffort::High;
    assert!(build_llm_provider(&config, None)
        .err()
        .unwrap()
        .to_string()
        .contains("2048"));
}
