pub mod auth;
pub mod error;
pub mod factory;
pub mod models;
pub mod provider_registry;
pub mod resolver;

pub(crate) mod convert;
mod providers;
pub(crate) mod wire_types;

pub use agent_contracts::{LlmProvider, ProviderCapabilities};
pub use agent_types::{
    AssistantMessage, LlmResponse, StopReason, StreamChunk, ToolUseBlock, Usage,
};
pub use agent_types::{ChatMessage, ContentBlock, MessageRole};
pub use agent_types::{CompletionConfig, LlmRequest, ResponseFormat, Tool, ToolChoice};
pub use error::LlmError;

pub use auth::{AuthCredential, AuthPool, AuthState, InMemoryAuthPool};
pub use auth::{AuthStore, AuthStoreError, FileAuthStore, InMemoryAuthStore};
pub use factory::{
    create_llm_provider, create_llm_provider_from_resolved, LlmProviderConfig, LlmProviderWrapper,
};
pub use models::{create_model_catalog, resolve_model_context_length, ModelCatalog, ModelSummary};
pub use provider_registry::{
    normalize_api_base, resolve_protocol_family, resolve_provider_profile, supported_providers,
    ApiBaseStyle, ProtocolFamily, ProviderProfile,
};
pub use providers::{OpenAiCompatibleProvider, OpenAiCompatibleProviderConfig};
pub use resolver::{resolve_config, ResolveError, ResolveInput, ResolvedConfig};
