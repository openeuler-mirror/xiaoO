use std::sync::{Arc, Mutex};

use crate::gateway::{
    AppBootstrap, AppTurnRequest, AppTurnResult, HostedSessionRuntimeConfig,
    HostedSessionRuntimeResolver, SessionControlPlane, SessionOpenRequest, SessionRuntimeBindings,
    SessionStore,
};
use crate::interaction_prompt::UserPromptResult;

use super::session::{
    ChannelInteractionHandle, ChannelLoopEventSink, ChannelToolEventSink, SessionGateway,
    SessionTurnUpdate,
};

impl SessionGateway {
    pub fn new() -> Self {
        Self {
            session_store: Arc::new(crate::gateway::InMemorySessionStore::default()),
        }
    }

    pub async fn ensure_session_open(
        &self,
        runtime_config: HostedSessionRuntimeConfig,
        request: SessionOpenRequest,
    ) -> Result<(), String> {
        let control_plane =
            self.build_control_plane(runtime_config, SessionRuntimeBindings::default())?;
        control_plane
            .open_session(request)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    pub fn spawn_turn(
        &self,
        runtime_config: HostedSessionRuntimeConfig,
        request: AppTurnRequest,
        updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>,
        interaction_rx: tokio::sync::mpsc::UnboundedReceiver<UserPromptResult>,
    ) {
        let session_store = Arc::clone(&self.session_store);
        tokio::spawn(async move {
            let loop_summary = Arc::new(Mutex::new(None));
            let bindings = SessionRuntimeBindings {
                loop_event_sink: Some(Arc::new(ChannelLoopEventSink::new(
                    updates_tx.clone(),
                    Arc::clone(&loop_summary),
                ))),
                tool_event_sink: Some(Arc::new(ChannelToolEventSink::new(updates_tx.clone()))),
                interaction_handle: Some(Arc::new(ChannelInteractionHandle::new(
                    updates_tx.clone(),
                    interaction_rx,
                ))),
                channel_file_sender: None,
            };

            let resolver = Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
            let dependencies = match AppBootstrap::from_session_components(session_store, resolver)
            {
                Ok(dependencies) => dependencies,
                Err(error) => {
                    let _ = updates_tx.send(SessionTurnUpdate::Err(error.to_string()));
                    return;
                }
            };

            let result = dependencies.session_service.run_turn(request).await;
            match result {
                Ok(AppTurnResult {
                    messages,
                    prompt_tokens,
                    completion_tokens,
                    total_tokens,
                    ..
                }) => {
                    let total_tokens = if total_tokens == 0 {
                        loop_summary
                            .lock()
                            .ok()
                            .and_then(|summary| summary.as_ref().map(|value| value.total_tokens))
                            .unwrap_or(0) as u64
                    } else {
                        total_tokens
                    };
                    let _ = updates_tx.send(SessionTurnUpdate::Done {
                        prompt_tokens,
                        completion_tokens,
                        total_tokens,
                        messages,
                    });
                }
                Err(error) => {
                    let _ = updates_tx.send(SessionTurnUpdate::Err(error.to_string()));
                }
            }
        });
    }

    fn build_control_plane(
        &self,
        runtime_config: HostedSessionRuntimeConfig,
        bindings: SessionRuntimeBindings,
    ) -> Result<Arc<dyn SessionControlPlane>, String> {
        let resolver = Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
        let session_store: Arc<dyn SessionStore> = self.session_store.clone();
        AppBootstrap::from_session_components(session_store, resolver)
            .map(|dependencies| dependencies.session_control_plane)
            .map_err(|error| error.to_string())
    }
}
