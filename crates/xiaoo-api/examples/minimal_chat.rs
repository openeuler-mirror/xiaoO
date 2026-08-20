//! Minimal pure-runtime example.
//!
//! The caller owns runtime state and injects runtime dependencies directly;
//! there is no host, session service, lease, or backend manager involved.

use std::sync::Arc;

use agent_types::context::TokenBudgetConfig;
use compact::{build_context_manager, CompactionPolicy};
use llm_client::{create_llm_provider, LlmProviderConfig};
use prompt::PromptBuilderImpl;
use tool::EmptyToolRegistry;
use xiaoo_api::prelude::*;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let provider_name = std::env::var("XIAOO_PROVIDER").unwrap_or_else(|_| "ollama".to_string());
    let model = std::env::var("XIAOO_MODEL").unwrap_or_else(|_| "qwen2.5:7b".to_string());
    let budget = TokenBudgetConfig {
        total_budget: 8_192,
        reserved_for_output: 1_024,
        reserved_for_system: 512,
        hard_limit_ratio: 0.9,
    };

    let provider = create_llm_provider(
        &LlmProviderConfig::new(provider_name, model),
        Some("example".to_string()),
        None,
    )?;
    let provider = Arc::new(provider);
    let compression = build_context_manager(None, provider.clone())?;
    let runtime = Runtime::builder()
        .llm_provider(provider)
        .compression_pipeline(compression)
        .prompt_builder(Arc::new(PromptBuilderImpl::new()))
        .system_prompt("You are a concise coding assistant.")
        .tool_registry(Arc::new(EmptyToolRegistry::new()))
        .skill_registry(Arc::new(xiaoo_core::EmptySkillRegistry::new()))
        .token_budget_config(budget.clone())
        .token_budget_policy(Arc::new(CompactionPolicy::from_budget(&budget)))
        .build()?;

    let mut state = runtime.new_state("example-conversation");
    let result = runtime
        .run(
            &mut state,
            RuntimeInput::new("Summarize the current project."),
        )
        .await?;
    match result {
        RuntimeOutput::Complete(_) => println!("completed"),
        RuntimeOutput::Suspended(calls) => println!("suspended tool calls: {}", calls.len()),
    }
    Ok(())
}
