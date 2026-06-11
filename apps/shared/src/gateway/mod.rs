pub mod backend;
pub mod bootstrap;
pub mod channel_interaction;
pub mod core_session_service;
pub mod decrypted_api_keys;
pub mod hosted_runtime_resolver;
pub mod pending_interaction;
pub mod permission_backend;
pub mod progress_updates;
pub mod prompt_utils;
pub mod runtime_bindings;
pub mod runtime_factory;
pub mod runtime_resolver;
mod session_backend;
pub mod session_base;
mod session_handle;
pub mod session_record;
pub mod session_service;
pub mod session_store;
pub mod session_supervisor;
pub mod session_worker;
pub mod subagent_interaction;
pub mod turns;
pub mod workspace_prompt;

pub use decrypted_api_keys::{get_decrypted_api_key, init_secret_provider, SecretProvider};

pub use bootstrap::{AppBootstrap, AppBootstrapError, AppDependencies};
pub use core_session_service::CoreBackedSessionService;
pub use hosted_runtime_resolver::{
    HostedSessionRuntimeConfig, HostedSessionRuntimeResolver, SubagentRoleConfigEntry,
};
pub use progress_updates::ChannelProgressRelayHandle;
pub use runtime_bindings::SessionRuntimeBindings;
pub use runtime_factory::{AppRuntimeAssembly, AppRuntimeFactory, AppRuntimeFactoryError};
pub use runtime_resolver::{
    ResolvedSessionRuntime, SessionRuntimeBuildInput, SessionRuntimeDescriptor,
    SessionRuntimeResolveError, SessionRuntimeResolver,
};
pub use session_base:: {channel_session_id, SessionInput, SessionInputKind, SessionOpenRequest, SessionSubmitReceipt} ;
pub use session_record::{SessionLifecycleStatus, SessionRecord};
pub use session_service::{SessionControlPlane, SessionService, SessionServiceError};
pub use session_store::{InMemorySessionStore, SessionStore, SessionStoreError};
pub use turns::{AppTurnRequest, GatewayEntryContext, GatewayEntryKind, TurnMention, AppTurnResult};
pub use workspace_prompt::compose_workspace_system_prompt;
