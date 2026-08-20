//! LLM provider 构建与上下文窗口解析、压缩管道。
//!
//! - [`create_llm_provider`] + [`LlmProviderWrapper`]：构建 provider 实例
//! - [`resolve_config`] + [`ResolveInput`]：解析 provider / model / api_base / api_key
//! - [`resolve_model_context_length`] / [`get_known_model_context_length`] /
//!   [`resolve_protocol_family`] / [`ProtocolFamily`]：上下文窗口探测链
//! - [`build_context_manager`] + [`CompactOverrides`]：构建压缩管道
//!
//! The resulting provider and compression pipeline can be injected directly
//! through [`crate::runtime::RuntimeBuilder`].

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
