//! LLM provider construction, context-window resolution, and the
//! compression pipeline.
//!
//! - [`create_llm_provider`] + [`LlmProviderWrapper`]: build a provider instance
//! - [`resolve_config`] + [`ResolveInput`]: resolve provider / model / api_base / api_key
//! - [`resolve_model_context_length`] / [`resolve_protocol_family`] /
//!   [`ProtocolFamily`]: context-window probing chain
//! - [`build_context_manager`] + [`CompactOverrides`]: build the compression pipeline
//!
//! The resulting provider and compression pipeline can be injected directly
//! through [`crate::runtime::RuntimeBuilder`].

// ---- Provider construction ----
#[doc(inline)]
pub use llm_client::{create_llm_provider, LlmProviderConfig, LlmProviderWrapper};

// ---- Provider / model resolution ----
#[doc(inline)]
pub use llm_client::{resolve_config, ResolveInput};

// ---- Provider registry ----
#[doc(inline)]
pub use llm_client::{
    provider_catalog, resolve_provider_profile, ProviderMetadata, ProviderProfile, ReasoningDialect,
};

// ---- Context-window probing chain ----
#[doc(inline)]
pub use llm_client::{resolve_model_context_length, resolve_protocol_family, ProtocolFamily};

// ---- Compression pipeline ----
#[doc(inline)]
pub use agent_contracts::{CompressionPipeline, LlmProvider, ProviderCapabilities};

#[doc(inline)]
pub use compact::{
    build_context_manager, CompactOverrides, ContextThresholds, DEFAULT_AUTO_COMPACT_RATIO,
    DEFAULT_BLOCKING_RATIO, DEFAULT_COLLAPSE_PRESERVE_TAIL, DEFAULT_SNIP_PRESERVE_TAIL,
    DEFAULT_SNIP_STALE_AFTER_MS, DEFAULT_SUMMARY_LLM_MAX_TOKENS, DEFAULT_SUMMARY_MAX_TOKENS,
    DEFAULT_SUMMARY_PRESERVE_TAIL, DEFAULT_WARNING_RATIO,
};

// ---- Token budget and compaction policy ----
#[doc(inline)]
pub use agent_contracts::TokenBudgetPolicy;

#[doc(inline)]
pub use compact::CompactionPolicy;
