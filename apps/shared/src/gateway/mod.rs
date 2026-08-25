mod backend_workspace_context;
pub mod bootstrap;
pub mod channel_interaction;
pub mod decrypted_api_keys;
mod e2b_runtime;
pub mod hook_action_sink;
pub use hook_action_sink::DaemonHookActionSink;
pub mod hosted_runtime_resolver;
pub mod llm_assembly;
pub mod memory_automation;
#[cfg(test)]
mod memory_automation_test;
pub mod pending_interaction;
pub mod permission_backend;
pub mod progress_updates;
pub mod prompt_utils;
mod session_backend;
pub mod session_base;
mod session_handle;
pub mod session_lease;
pub use session_lease::{
    daemon_channel_principal, daemon_cron_principal, daemon_hook_principal, is_daemon_principal,
    STALE_LEASE_THRESHOLD_MS,
};
pub(crate) use session_lease::{SessionLeaseTable, ORPHAN_SESSION_THRESHOLD_MS, REAPER_INTERVAL};
pub mod session_record;
pub mod session_runtime;
pub mod session_service;
pub mod session_service_impl;
pub mod session_store;
pub mod session_supervisor;
pub mod session_worker;
pub mod subagent_interaction;
pub mod tool_assembly;
pub use tool_assembly::{BoundControlStore, McpToolCache, ToolAssemblyError};
pub mod turns;
pub mod workspace_prompt;

pub use decrypted_api_keys::{get_decrypted_api_key, init_secret_provider, SecretProvider};
pub(crate) use e2b_runtime::finalize_e2b_runtime;

pub use bootstrap::{AppBootstrap, AppBootstrapError, AppDependencies};
pub use hosted_runtime_resolver::{
    HostedSessionRuntimeConfig, HostedSessionRuntimeResolver, SubagentRoleConfigEntry,
};
pub use memory_automation::{
    McpMemoryAutomation, MemoryAutomationConfig, MemoryAutomationHealth, TurnMemoryAutomation,
};
pub use progress_updates::ChannelProgressRelayHandle;
pub(crate) use session_base::SessionHeartbeatRequest;
pub use session_base::{
    channel_session_id, RuntimeCancelRequest, RuntimeCloseRequest, RuntimeDetachRequest,
    RuntimeHeartbeatRequest, RuntimeInteractionRequest, RuntimeOpenRequest, SessionDetachRequest,
    SessionInput, SessionInputKind, SessionOpenRequest, SessionSubmitReceipt,
};
pub use session_record::{
    RuntimeBootstrapBinding, RuntimeBootstrapSkill, SessionAgentRecord, SessionLifecycleStatus,
    SessionRecord, SessionRuntimeSnapshot, E2B_BOOTSTRAP_MANIFEST_VERSION,
    E2B_REMOTE_WORKSPACE_ROOT,
};
pub(crate) use session_record::{SessionStateOutcome, E2B_REMOTE_SKILLS_ROOT};
pub(crate) use session_runtime::AppRuntimeFactoryError;
pub use session_runtime::{
    AppRuntimeFactory, ResolvedSessionRuntime, SessionRuntimeBindings, SessionRuntimeBuildInput,
    SessionRuntimeDescriptor, SessionRuntimeResolveError, SessionRuntimeResolver,
};
pub use session_service::{SessionControlPlane, SessionService, SessionServiceError};
pub use session_service_impl::CoreBackedSessionService;
pub use session_store::{InMemorySessionStore, SessionStore, SessionStoreError};
pub use turns::{
    AppTurnRequest, AppTurnResult, GatewayEntryContext, GatewayEntryKind, LlmRuntimeConfig,
    RuntimeTurnRequest, TurnMention, TurnOutcome,
};
pub(crate) use workspace_prompt::compose_repo_map;
pub use workspace_prompt::compose_workspace_system_prompt;
