//! LLM provider 构建与上下文窗口解析、压缩管道。
//!
//! 【L2 Advanced 层】
//!
//! 常规路径下这些函数全部由 [`crate::host::SessionOptions`]（阶段 2 落地）
//! 派生内部调用（`refactor.md` §3.3.9）：
//!
//! - [`create_llm_provider`] + [`LlmProviderWrapper`]：构建 provider 实例
//! - [`resolve_config`] + [`ResolveInput`]：解析 provider / model / api_base / api_key
//! - [`resolve_model_context_length`] / [`get_known_model_context_length`] /
//!   [`resolve_protocol_family`] / [`ProtocolFamily`]：上下文窗口探测链
//! - [`build_context_manager`] + [`CompactOverrides`]：构建压缩管道
//!
//! 本模块保留导出供特殊装配（自定义 provider 包装、独立探测工具）。

// ---- provider 构建 ----
#[doc(inline)]
pub use llm_client::{create_llm_provider, LlmProviderConfig, LlmProviderWrapper};

// ---- provider / model 解析 ----
#[doc(inline)]
pub use llm_client::{resolve_config, ResolveInput};

// ---- 上下文窗口探测链 ----
#[doc(inline)]
pub use llm_client::{
    get_known_model_context_length, resolve_model_context_length, resolve_protocol_family,
    ProtocolFamily,
};

// ---- 压缩管道 ----
#[doc(inline)]
pub use agent_contracts::{CompressionPipeline, LlmProvider, ProviderCapabilities};

#[doc(inline)]
pub use compact::{build_context_manager, CompactOverrides};
