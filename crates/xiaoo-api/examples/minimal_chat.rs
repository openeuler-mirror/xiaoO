//! Minimal pure-runtime example.
//!
//! Only `xiaoo_api` is imported — no host, session service, lease, backend
//! manager, or lower-crate direct references.  The caller owns runtime state
//! and injects the two required dependencies (`llm_provider` +
//! `token_budget_config`); every other runtime dependency falls back to the
//! SDK standard defaults.

use std::sync::Arc;

use xiaoo_api::chat::TokenBudgetConfig;
use xiaoo_api::llm::{create_llm_provider, LlmProviderConfig};
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

    let provider = Arc::new(create_llm_provider(
        &LlmProviderConfig::new(provider_name, model),
        Some("example".to_string()),
        None,
    )?);
    let runtime = Runtime::builder()
        .llm_provider(provider)
        .system_prompt("You are a concise coding assistant.")
        .token_budget_config(budget)
        .build()?; // 其余 6 项全走缺省

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
