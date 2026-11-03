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
    /// `None` unless the bootstrap ran with [`ReaperPolicy::Enabled`].
    /// Dropping it does NOT abort the task; `.abort()` or runtime drop stops it.
    pub reaper_handle: Option<tokio::task::JoinHandle<()>>,
}

/// Whether the assembled process should run the orphan-session reaper.
///
/// The reaper's orphan criteria assume the daemon's attach-lease model: the
/// session store is shared by multiple client processes, leases are refreshed
/// over HTTP, and "no live lease" reliably means "no holder". Only the daemon
/// fits that model. A single-process local TUI/CLI owns its store for its own
/// process lifetime and never produces leases, so the same criteria would
/// delete the records of sessions whose only holder — the process itself —
/// is still alive after 2 h of idle time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReaperPolicy {
    /// Spawn the reaper. For processes serving multiple clients over a store
    /// that outlives any one of them (the daemon): reclaims leaked backends
    /// and drops records whose lease-holding client is gone.
    Enabled,
    /// Do not spawn the reaper. For single-process local mode: the store dies
    /// with the process, so there is no orphan to reclaim and reaping by
    /// wall-clock idle time can only destroy live sessions' records.
    Disabled,
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
    /// so `run_turn` must never be called on the returned dependencies. Runs no
    /// orphan reaper: lifecycle control planes are TUI-local, and the TUI's
    /// store is process-lifetime (see [`ReaperPolicy`]).
    pub fn lifecycle_only(
        session_store: Arc<dyn SessionStore>,
        hooker_config: HookerRegistryConfig,
        backend_manager: Arc<BackendManager>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        let resolver: Arc<dyn SessionRuntimeResolver> = Arc::new(NoopSessionRuntimeResolver);
        Self::from_session_components_with_hooks_and_backend_manager_and_memory_automation(
            session_store,
            resolver,
            hooker_config,
            backend_manager,
            None,
            None,
            ReaperPolicy::Disabled,
        )
    }

    /// Local-mode assembly. Equivalent to the long form below with no subagent
    /// interaction timeout and no orphan reaper. Every current caller is a
    /// single-process local TUI/CLI path whose store dies with the process;
    /// the daemon must use the long form with [`ReaperPolicy::Enabled`] so
    /// leaked backends are reclaimed.
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
            ReaperPolicy::Disabled,
        )
    }

    pub fn from_session_components_with_hooks_and_backend_manager_and_memory_automation(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
        hooker_config: HookerRegistryConfig,
        backend_manager: Arc<BackendManager>,
        memory_automation: Option<Arc<dyn crate::gateway::TurnMemoryAutomation>>,
        interaction_timeout: Option<std::time::Duration>,
        reaper_policy: ReaperPolicy,
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
        // Best-effort orphan-session reaper (see [`ReaperPolicy`] for why
        // only daemon-style assembly may run it): scans `list_all()` every
        // 10 min and force-closes sessions whose lease has been stale for
        // 45 s and whose last activity is older than 2 hours.
        let reaper_handle = match reaper_policy {
            ReaperPolicy::Enabled => Some(session_components.spawn_orphan_reaper()),
            ReaperPolicy::Disabled => None,
        };
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
