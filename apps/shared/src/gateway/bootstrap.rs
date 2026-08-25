use crate::backend::BackendManager;
use crate::gateway::{
    CoreBackedSessionService, ResolvedSessionRuntime, SessionControlPlane,
    SessionRuntimeBuildInput, SessionRuntimeResolveError, SessionRuntimeResolver, SessionService,
    SessionStore,
};
use agent_types::hook::HookerRegistryConfig;
use async_trait::async_trait;
use hook::framework::HookerRegistryBuilderImpl;
use hook::HookerRegistryBuilder;
use std::sync::Arc;
use thiserror::Error;

pub struct AppDependencies {
    pub session_service: Arc<dyn SessionService>,
    pub session_control_plane: Arc<dyn SessionControlPlane>,
    pub backend_manager: Arc<BackendManager>,
    /// Handle to the orphan-session reaper (best-effort background task that
    /// force-closes sessions whose attach lease has been gone for ~2 hours).
    /// Dropping it does NOT abort the task; `.abort()` or runtime drop stops it.
    pub reaper_handle: Option<tokio::task::JoinHandle<()>>,
}

pub struct AppBootstrap;

#[derive(Debug, Error)]
pub enum AppBootstrapError {
    #[error("bootstrap requires a session store")]
    MissingSessionStore,
    #[error("bootstrap requires a session runtime resolver")]
    MissingRuntimeResolver,
    #[error("failed to build session hooker registry: {0}")]
    HookerBuild(#[from] agent_types::common::BuildError),
}

struct NoopSessionRuntimeResolver;

#[async_trait]
impl SessionRuntimeResolver for NoopSessionRuntimeResolver {
    async fn resolve(
        &self,
        _request: &SessionRuntimeBuildInput,
        _existing: Option<&crate::gateway::SessionRecord>,
    ) -> Result<ResolvedSessionRuntime, SessionRuntimeResolveError> {
        Err(SessionRuntimeResolveError::ResolveFailed {
            message: "lifecycle-only control plane cannot resolve runtimes".to_string(),
        })
    }
}

impl AppBootstrap {
    /// Builds a control plane that is only suitable for session lifecycle hooks
    /// (open/close). It shares the given session_store but uses a noop resolver,
    /// so `run_turn` must never be called on the returned dependencies.
    pub fn lifecycle_only(
        session_store: Arc<dyn SessionStore>,
        hooker_config: HookerRegistryConfig,
        backend_manager: Arc<BackendManager>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        let resolver: Arc<dyn SessionRuntimeResolver> = Arc::new(NoopSessionRuntimeResolver);
        Self::from_session_components_with_hooks_and_backend_manager(
            session_store,
            resolver,
            hooker_config,
            backend_manager,
        )
    }

    #[allow(dead_code)]
    pub(crate) fn from_session_components(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        Self::from_session_components_with_hooks(
            session_store,
            runtime_resolver,
            HookerRegistryConfig::default(),
        )
    }

    #[allow(dead_code)]
    pub(crate) fn from_session_components_with_hooks(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_config: HookerRegistryConfig,
    ) -> Result<AppDependencies, AppBootstrapError> {
        Self::from_session_components_with_hooks_and_backend_manager(
            session_store,
            runtime_resolver,
            hooker_config,
            Arc::new(BackendManager::new()),
        )
    }

    pub fn from_session_components_with_hooks_and_backend_manager(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_config: HookerRegistryConfig,
        backend_manager: Arc<BackendManager>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        Self::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
            session_store,
            runtime_resolver,
            hooker_config,
            backend_manager,
            None,
            // No subagent interaction timeout for the short-form bootstrap;
            // callers that need the cap (the daemon) use the long form.
            None,
        )
    }

    pub fn from_session_components_with_hooks_and_backend_manager_and_memory_automation(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_config: HookerRegistryConfig,
        backend_manager: Arc<BackendManager>,
        memory_automation: Option<Arc<dyn crate::gateway::TurnMemoryAutomation>>,
        interaction_timeout: Option<std::time::Duration>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        // Extract the cross-turn `send_prompt` chain depth cap before
        // `hooker_config` is consumed by the registry builder. The cap is
        // enforced by `CoreBackedSessionService::fire_session_state_hook_and_collect_actions`,
        // not by the hooker registry itself.
        let max_prompt_chain_depth = hooker_config.max_prompt_chain_depth;
        let hooker_registry = HookerRegistryBuilderImpl::new()
            .with_config(hooker_config)
            .build()?;
        let session_components = Arc::new(CoreBackedSessionService::new(
            session_store,
            runtime_resolver.clone(),
            Arc::from(hooker_registry),
            Arc::clone(&backend_manager),
            max_prompt_chain_depth,
            memory_automation,
        ));
        // Opt-in strict lease enforcement for anonymous callers via
        // `XIAOO_ENFORCE_LEASE` (truthy: `1` / `true` / `yes` / `on`).
        // When on, mutating RPCs without `client_id` are rejected with
        // `LeaseRequired` (HTTP 401). Default off for gradual rollout.
        let enforce = std::env::var("XIAOO_ENFORCE_LEASE")
            .ok()
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);
        session_components.set_enforce_anonymous_lease(enforce);
        // Forward the configured interaction timeout (the daemon forwards
        // the same `interaction_timeout_secs` it uses for the HTTP router)
        // so a subagent's `ask_user_question` forwarded via
        // `request_interaction` cannot hang forever waiting for the user
        // (for handles without a built-in timeout, e.g.
        // `RemoteSseInteractionHandle`). `None` = no outer cap (rely on the
        // handle's own timeout, or block until the channel closes).
        session_components.set_interaction_timeout(interaction_timeout);
        // Log the enforcement mode so operators can tell whether anonymous
        // callers are actually single-writer-guarded.
        let anonymous_lease_enforced = session_components.anonymous_lease_enforced();
        tracing::info!(
            enforce_anonymous_lease = anonymous_lease_enforced,
            "attach-lease enforcement mode: anonymous callers are {}",
            if anonymous_lease_enforced {
                "REJECTED (strict)"
            } else {
                "ALLOWED (bypass single-writer for rollout)"
            }
        );
        // Best-effort orphan-session reaper: scans `list_all()` every 10 min
        // and force-closes sessions whose lease has been gone for ~2 hours.
        let reaper_handle = Some(session_components.spawn_orphan_reaper());
        let subagent_control: Arc<dyn subagent::SubagentControl> = session_components.clone();
        // Prefer the resolver's opaque store when available (so the control is
        // reused by coarse-grained tool assembly that the resolver delegates to
        // without the resolver itself naming the control trait).
        if let Some(store) = runtime_resolver.bound_control_store() {
            store.set(subagent_control.clone());
        }
        runtime_resolver.bind_subagent_control(subagent_control);
        let session_service: Arc<dyn SessionService> = session_components.clone();
        let session_control_plane: Arc<dyn SessionControlPlane> = session_components;
        Ok(AppDependencies {
            session_service,
            session_control_plane,
            backend_manager,
            reaper_handle,
        })
    }
}
