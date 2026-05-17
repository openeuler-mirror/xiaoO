use std::sync::{Arc, Mutex};

use crate::gateway::{
    AppBootstrap, AppDependencies, AppTurnRequest, AppTurnResult, HostedSessionRuntimeConfig,
    HostedSessionRuntimeResolver, SessionControlPlane, SessionOpenRequest, SessionRecord,
    SessionRuntimeBindings, SessionStore,
};
use crate::interaction_prompt::UserPromptResult;

use super::session::{
    ChannelInteractionHandle, ChannelLoopEventSink, ChannelToolEventSink, SessionGateway,
    SessionTurnUpdate,
};

impl SessionGateway {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns (or lazily initialises) the lifecycle-only control plane.
    /// The lifecycle control plane shares `session_store` with all per-turn
    /// bootstraps, but uses a noop resolver — safe to call only for
    /// `open_session` / `force_close_session`.
    async fn get_or_init_lifecycle_control_plane(
        &self,
        hooker_config: agent_types::hook::HookerRegistryConfig,
    ) -> Result<Arc<dyn SessionControlPlane>, String> {
        let mut lock = self.lifecycle_control_plane.lock().await;
        if let Some(cp) = lock.as_ref() {
            return Ok(cp.clone());
        }
        let store: Arc<dyn SessionStore> = self.session_store.clone();
        let deps = AppBootstrap::lifecycle_only(store, hooker_config)
            .map_err(|error| error.to_string())?;
        let cp = deps.session_control_plane;
        *lock = Some(cp.clone());
        Ok(cp)
    }

    pub async fn ensure_session_open(
        &self,
        runtime_config: HostedSessionRuntimeConfig,
        request: SessionOpenRequest,
    ) -> Result<(), String> {
        let session_id = request.session_id.clone();
        let hooker_config = runtime_config.hooker.clone();

        // open_session calls resolve() to build the SessionRecord, so it needs a
        // real resolver. The lifecycle-only CP uses NoopSessionRuntimeResolver and
        // must not be used here; it is only safe for force_close_session (close_all_sessions).
        self.build_control_plane(runtime_config, SessionRuntimeBindings::default())?
            .session_control_plane
            .open_session(request)
            .await
            .map_err(|error| error.to_string())?;

        // Ensure the lifecycle CP is ready for close_all_sessions.
        self.get_or_init_lifecycle_control_plane(hooker_config)
            .await?;
        self.active_session_ids.lock().await.insert(session_id);
        Ok(())
    }

    pub async fn session_snapshot(&self, session_id: &str) -> Option<SessionRecord> {
        self.session_store.load(session_id).await
    }

    pub async fn import_session_snapshot(&self, record: SessionRecord) {
        let session_id = record.session_id.clone();

        let chunk_hashes: Vec<String> = record
            .loop_state
            .as_ref()
            .map(|ls| ls.kv_cache_map.keys().cloned().collect())
            .unwrap_or_default();

        self.session_store.save(record).await;
        self.active_session_ids.lock().await.insert(session_id.clone());

        if !chunk_hashes.is_empty() {
            let count = chunk_hashes.len();
            let sid = session_id;
            tokio::spawn(async move {
                let payload = serde_json::json!({
                    "chunk_hashes": chunk_hashes,
                    "lookup_id": "snapshot_prefetch",
                });
                match reqwest::Client::new()
                    .post("http://localhost:6999/memory/prefetch")
                    .json(&payload)
                    .send()
                    .await
                {
                    Ok(resp) => {
                        let status = resp.status();
                        let body = resp.text().await.unwrap_or_default();
                        let debug_entry = serde_json::json!({
                            "session_id": sid,
                            "request": payload,
                            "response_status": status.as_u16(),
                            "response_body": body,
                        });
                        let dir = std::path::Path::new("kvcache_debug");
                        let _ = std::fs::create_dir_all(dir);
                        let path = dir.join(format!("prefetch_debug_{}.json", sid));
                        if let Ok(json) = serde_json::to_string_pretty(&debug_entry) {
                            let _ = std::fs::write(&path, json);
                            tracing::info!(path = %path.display(), "prefetch debug file written");
                        }
                        tracing::info!(
                            status = %status,
                            count = count,
                            "kv cache prefetch completed"
                        );
                    }
                    Err(e) => {
                        let debug_entry = serde_json::json!({
                            "session_id": sid,
                            "request": payload,
                            "error": e.to_string(),
                        });
                        let dir = std::path::Path::new("kvcache_debug");
                        let _ = std::fs::create_dir_all(dir);
                        let path = dir.join(format!("prefetch_debug_{}.json", sid));
                        if let Ok(json) = serde_json::to_string_pretty(&debug_entry) {
                            let _ = std::fs::write(&path, json);
                            tracing::info!(path = %path.display(), "prefetch debug file written");
                        }
                        tracing::warn!(
                            error = %e,
                            count = count,
                            "kv cache prefetch failed"
                        );
                    }
                }
            });
        }
    }

    pub fn spawn_turn(
        &self,
        runtime_config: HostedSessionRuntimeConfig,
        request: AppTurnRequest,
        updates_tx: tokio::sync::mpsc::UnboundedSender<SessionTurnUpdate>,
        interaction_rx: tokio::sync::mpsc::UnboundedReceiver<UserPromptResult>,
    ) {
        let session_store: Arc<dyn SessionStore> = self.session_store.clone();
        // Track the session so close_all_sessions covers it even if
        // ensure_session_open was not called first.
        let active_session_ids = Arc::clone(&self.active_session_ids);
        let session_id = request.session_id.clone();
        tokio::spawn(async move {
            active_session_ids.lock().await.insert(session_id);

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

            let hooker_config = runtime_config.hooker.clone();
            let resolver = Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
            let dependencies = match AppBootstrap::from_session_components_with_hooks(
                session_store,
                resolver,
                hooker_config,
            ) {
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
                    estimated_input_tokens,
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
                        estimated_input_tokens,
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
    ) -> Result<AppDependencies, String> {
        let hooker_config = runtime_config.hooker.clone();
        let resolver = Arc::new(HostedSessionRuntimeResolver::new(runtime_config, bindings));
        AppBootstrap::from_session_components_with_hooks(
            self.session_store.clone(),
            resolver,
            hooker_config,
        )
        .map_err(|error| error.to_string())
    }

    /// Closes all tracked sessions via the lifecycle control plane,
    /// firing the SessionClosed hook for each.
    pub async fn close_all_sessions(&self) {
        let ids: Vec<String> = {
            let mut lock = self.active_session_ids.lock().await;
            let ids: Vec<_> = lock.iter().cloned().collect();
            lock.clear();
            ids
        };
        if ids.is_empty() {
            return;
        }
        let cp = self.lifecycle_control_plane.lock().await.clone();
        let Some(control_plane) = cp else {
            return;
        };
        for session_id in ids {
            if let Err(err) = control_plane.force_close_session(&session_id).await {
                tracing::warn!(
                    session_id = %session_id,
                    error = %err,
                    "failed to close session on exit"
                );
            }
        }
    }
}
