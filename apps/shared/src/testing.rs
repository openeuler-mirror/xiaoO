//! 面向应用测试的现成桩（`test-support` feature 门控）。
//!
//! 应用测试用它，无需自己 `impl LlmProvider`——也就不需要 `LlmRequest` /
//! `LlmResponse` / `StreamChunk` / `LlmError` 等底层词汇进入应用测试代码。
//! 这些词汇仅在本模块内部使用，不向应用导出。

#![cfg(feature = "test-support")]

use std::sync::Arc;

use agent_contracts::{LlmProvider, ProviderCapabilities};
use agent_types::{LlmError, LlmRequest, LlmResponse, StreamChunk};
use async_trait::async_trait;
use xiaoo_api::llm::LlmProviderWrapper;

/// 现成的 `LlmProvider` 测试桩，capabilities 可参数化。
///
/// `complete` / `complete_stream` 均 `unimplemented!`：桩仅用于探测上下文窗口
/// 与 provider 包装层路径，不实际执行推理（与原 endside `cli/mod.rs` 测试
/// 手搓的 `DummyProvider` 行为一致）。返回 `Arc<LlmProviderWrapper>`，可直接
/// 注入需要该类型的装配路径。
pub fn stub_llm_provider(model_name: &str, max_context_window: u32) -> Arc<LlmProviderWrapper> {
    Arc::new(LlmProviderWrapper::new(
        Arc::new(StubProvider {
            capabilities: ProviderCapabilities {
                supports_streaming: false,
                supports_tool_calls: false,
                supports_json_mode: false,
                max_context_window: max_context_window as usize,
                model_name: model_name.to_string(),
            },
        }),
        None,
        None,
    ))
}

struct StubProvider {
    capabilities: ProviderCapabilities,
}

#[async_trait]
impl LlmProvider for StubProvider {
    async fn complete(&self, _request: &LlmRequest) -> Result<LlmResponse, LlmError> {
        unimplemented!("stub provider is not meant to run inference")
    }

    async fn complete_stream(
        &self,
        _request: &LlmRequest,
        _on_chunk: &(dyn Fn(StreamChunk) + Send + Sync),
    ) -> Result<LlmResponse, LlmError> {
        unimplemented!("stub provider is not meant to run inference")
    }

    fn capabilities(&self) -> &ProviderCapabilities {
        &self.capabilities
    }
}
