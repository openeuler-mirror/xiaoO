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

    pub fn from_session_components(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        Self::from_session_components_with_hooks(
            session_store,
            runtime_resolver,
            HookerRegistryConfig::default(),
        )
    }

    pub fn from_session_components_with_hooks(
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
        ));
        runtime_resolver.bind_subagent_control(
            session_components.clone() as Arc<dyn subagent::SubagentControl>,
        );
        let session_service: Arc<dyn SessionService> = session_components.clone();
        let session_control_plane: Arc<dyn SessionControlPlane> = session_components;
        Ok(AppDependencies {
            session_service,
            session_control_plane,
            backend_manager,
        })
    }
}
