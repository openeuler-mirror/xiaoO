//! # minimal_chat —— xiaoo-api 最小可执行用例
//!
//! 一个完整的对话程序（约 40 行），只 `use xiaoo_api::prelude::*`：
//! 建宿主 → 开会话 → 流式对话（含审批应答）→ 收尾。
//!
//! 这是 `refactor.md` §3.3.1 / §6 验收标准之一：
//! > `examples/minimal_chat.rs` 提交并可运行：完整"建宿主 → 开会话 → 流式对话
//! > （含审批应答）→ 收尾"不超过 40 行，且只 `use xiaoo_api::prelude::*`。
//!
//! ## 运行方式
//!
//! 本示例需要真实 LLM（默认用 ollama 本地模型）。先确保 ollama 在
//! `http://localhost:11434` 运行并已 `ollama pull qwen2.5:7b`，然后：
//!
//! ```bash
//! cargo run -p xiaoo-api --example minimal_chat
//! ```
//!
//! 也可通过环境变量切换 provider / model / api_key：
//!
//! ```bash
//! XIAOO_PROVIDER=openai XIAOO_MODEL=gpt-4.1 XIAOO_API_KEY=sk-... \
//!   cargo run -p xiaoo-api --example minimal_chat
//! ```

use std::time::Duration;
use xiaoo_api::prelude::*;
use xiaoo_api::interaction::{InteractionRequest, InteractionResponse};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // 从环境变量读 provider / model / api_key，缺省用 ollama 本地模型。
    let provider = std::env::var("XIAOO_PROVIDER").unwrap_or_else(|_| "ollama".to_string());
    let model = std::env::var("XIAOO_MODEL").unwrap_or_else(|_| "qwen2.5:7b".to_string());
    let api_base = std::env::var("XIAOO_API_BASE").ok();
    let api_key = std::env::var("XIAOO_API_KEY").ok();

    // 1. 建宿主：secrets 初始化、hooker、backend、memory automation 全部走缺省。
    let host = LocalSessionHost::builder().build().await?;

    // 2. 开会话：只写与缺省不同的配置，其余 60+ 字段由 API 内部派生。
    let mut llm = LlmOptions::new(&provider, &model);
    if let Some(base) = api_base {
        llm = llm.api_base(&base);
    }
    if let Some(key) = api_key {
        llm = llm.api_key(&key);
    }
    let session = host.open_session(SessionOptions::new(llm)).await?;

    // 3. 跑一轮对话，流式消费事件。
    let mut turn = session.run_turn("总结一下当前目录的代码结构").await?;
    while let Some(event) = turn.next_event().await {
        match event {
            TurnEvent::Text { delta, .. } => print!("{delta}"),
            TurnEvent::Interaction { request, responder } => {
                // 工具审批/提问：自动应答（生产场景应由用户交互决定）。
                responder.respond(auto_answer(&request));
            }
            _ => {}
        }
    }
    let result = turn.result().await?;
    println!("\n[tokens: total={}, prompt={}, completion={}]",
             result.total_tokens, result.prompt_tokens, result.completion_tokens);

    // 4. 收尾。
    session.close().await?;
    host.shutdown(Duration::from_secs(10)).await;
    Ok(())
}

/// 交互请求的自动应答（示例用——生产场景应由 UI/用户决定）。
fn auto_answer(request: &InteractionRequest) -> InteractionResponse {
    match request {
        InteractionRequest::Confirm { .. } => InteractionResponse::Confirmed { allowed: true },
        InteractionRequest::TextInput { .. } => InteractionResponse::Text {
            value: Some("ok".to_string()),
            display_value: None,
        },
        InteractionRequest::Choice { options, .. } => InteractionResponse::Choice {
            value: options.first().cloned(),
        },
    }
}
