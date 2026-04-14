use crate::gateway::{
    CoreBackedSessionService, SessionControlPlane, SessionRuntimeResolver, SessionService,
    SessionStore,
};
use std::sync::Arc;
use thiserror::Error;

pub struct AppDependencies {
    pub session_service: Arc<dyn SessionService>,
    pub session_control_plane: Arc<dyn SessionControlPlane>,
}

pub struct AppBootstrap;

#[derive(Debug, Error)]
pub enum AppBootstrapError {
    #[error("bootstrap requires a session store")]
    MissingSessionStore,
    #[error("bootstrap requires a session runtime resolver")]
    MissingRuntimeResolver,
}

impl AppBootstrap {
    pub fn from_session_components(
        session_store: Arc<dyn SessionStore>,
        runtime_resolver: Arc<dyn SessionRuntimeResolver>,
    ) -> Result<AppDependencies, AppBootstrapError> {
        let session_components = Arc::new(CoreBackedSessionService::new(
            session_store,
            runtime_resolver.clone(),
        ));
        runtime_resolver.bind_subagent_control(
            session_components.clone() as Arc<dyn subagent::SubagentControl>,
        );
        let session_service: Arc<dyn SessionService> = session_components.clone();
        let session_control_plane: Arc<dyn SessionControlPlane> = session_components;
        Ok(AppDependencies {
            session_service,
            session_control_plane,
        })
    }
}
