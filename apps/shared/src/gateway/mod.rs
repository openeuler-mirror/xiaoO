pub mod bootstrap;
pub mod channel_interaction;
pub mod session_service_impl;
pub mod decrypted_api_keys;
pub mod hosted_runtime_resolver;
pub mod pending_interaction;
pub mod permission_backend;
pub mod progress_updates;
pub mod prompt_utils;
mod session_backend;
pub mod session_base;
mod session_handle;
pub mod session_record;
pub mod session_runtime;
pub mod session_service;
pub mod session_store;
pub mod session_supervisor;
pub mod session_worker;
pub mod subagent_interaction;
pub mod turns;
pub mod workspace_prompt;

pub use decrypted_api_keys::{get_decrypted_api_key, init_secret_provider, SecretProvider};

pub use bootstrap::{AppBootstrap, AppBootstrapError, AppDependencies};
pub use session_service_impl::CoreBackedSessionService;
pub use hosted_runtime_resolver::{
    HostedSessionRuntimeConfig, HostedSessionRuntimeResolver, SubagentRoleConfigEntry,
};
pub use progress_updates::ChannelProgressRelayHandle;
pub use session_base::{
    channel_session_id, SessionCancelRequest, SessionCloseRequest, SessionForkRequest,
    SessionForkResult, SessionInput, SessionInputKind, SessionInteractionRequest,
    SessionOpenRequest, SessionSubmitReceipt,
};
pub use session_record::{SessionLifecycleStatus, SessionRecord};
pub use session_runtime::{
    AppRuntimeAssembly, AppRuntimeFactory, AppRuntimeFactoryError, ResolvedSessionRuntime,
    SessionRuntimeBindings, SessionRuntimeBuildInput, SessionRuntimeDescriptor,
    SessionRuntimeResolveError, SessionRuntimeResolver,
};
pub use session_service::{SessionControlPlane, SessionService, SessionServiceError};
pub use session_store::{InMemorySessionStore, SessionStore, SessionStoreError};
pub use turns::{
    AppTurnRequest, AppTurnResult, GatewayEntryContext, GatewayEntryKind, LlmRuntimeConfig,
    TurnMention,
};
pub use workspace_prompt::compose_workspace_system_prompt;
